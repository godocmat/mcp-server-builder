use std::io::{self, BufRead, Read, Write};
use std::process::Stdio;
use std::time::{Duration, Instant};

use rmcp::model::{ErrorCode, ProtocolVersion};
use rmcp::service::{ClientInitializeError, ClientLifecycleMode, ClientServiceExt};
use rmcp::transport::async_rw::AsyncRwTransport;
use serde_json::{Value, json};
use tokio::process::{ChildStdin, ChildStdout, Command};
use tokio::time::timeout;

const MODERN: &str = "2026-07-28";
const LEGACY: &str = "2025-11-25";
const OLDER: &str = "2025-03-26";
const OTHER_MODERN: &str = "2099-01-01";
const FIXTURE_ARG: &str = "--client-auto-fixture";
const CASE_LIMIT: Duration = Duration::from_secs(20);
const SHUTDOWN_LIMIT: Duration = Duration::from_secs(3);

#[derive(Clone, Copy)]
enum Scenario {
    LegacyError,
    LegacySilence,
    ModernRetry,
    ModernNoCommonVersion,
    ModernCapabilityError,
    OlderLegacyReply,
}

const SCENARIOS: &[(&str, Scenario)] = &[
    ("legacy-error", Scenario::LegacyError),
    ("legacy-silence", Scenario::LegacySilence),
    ("modern-version-retry", Scenario::ModernRetry),
    ("modern-no-common-version", Scenario::ModernNoCommonVersion),
    ("modern-capability-error", Scenario::ModernCapabilityError),
    ("older-legacy-reply", Scenario::OlderLegacyReply),
];

#[tokio::main(flavor = "current_thread")]
async fn main() -> io::Result<()> {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() == Some(FIXTURE_ARG) {
        let name = args
            .next()
            .ok_or_else(|| io::Error::other("missing fixture scenario"))?;
        let scenario = SCENARIOS
            .iter()
            .find_map(|(key, scenario)| (*key == name).then_some(*scenario))
            .ok_or_else(|| io::Error::other("unknown fixture scenario"))?;
        return raw_peer(scenario, io::stdin().lock(), io::stdout().lock());
    }

    for &(name, scenario) in SCENARIOS {
        let elapsed = run_case(name, scenario)
            .await
            .map_err(|error| io::Error::other(format!("{name}: {error}")))?;
        println!("passed {name} ({elapsed:.2?})");
    }
    Ok(())
}

async fn run_case(name: &str, scenario: Scenario) -> io::Result<Duration> {
    let mut child = Command::new(std::env::current_exe()?)
        .args([FIXTURE_ARG, name])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let transport = AsyncRwTransport::new_client(
        child.stdout.take().expect("piped stdout"),
        child.stdin.take().expect("piped stdin"),
    );
    let outcome = async {
        let elapsed = timeout(CASE_LIMIT, verify_client(scenario, transport))
            .await
            .map_err(|_| io::Error::other("SDK client timed out"))??;
        let status = timeout(SHUTDOWN_LIMIT, child.wait())
            .await
            .map_err(|_| io::Error::other("fixture did not exit after EOF"))??;
        check(status.success(), "fixture rejected the wire exchange")?;
        Ok(elapsed)
    }
    .await;
    if outcome.is_err() {
        let _ = child.start_kill();
        let _ = timeout(SHUTDOWN_LIMIT, child.wait()).await;
    }
    outcome
}

async fn verify_client(
    scenario: Scenario,
    transport: AsyncRwTransport<rmcp::RoleClient, ChildStdout, ChildStdin>,
) -> io::Result<Duration> {
    let mut preferred_versions = vec![ProtocolVersion::V_2026_07_28];
    if matches!(scenario, Scenario::ModernRetry) {
        preferred_versions.insert(0, serde_json::from_value(json!(OTHER_MODERN))?);
    }
    let started = Instant::now();
    let connection = ()
        .serve_with_lifecycle(
            transport,
            ClientLifecycleMode::Auto {
                preferred_versions,
                legacy_version: Some(ProtocolVersion::V_2025_11_25),
            },
        )
        .await;
    let elapsed = started.elapsed();
    let mut client = match (scenario, connection) {
        (
            Scenario::ModernNoCommonVersion,
            Err(ClientInitializeError::NoCompatibleProtocolVersion {
                client_supported,
                server_supported,
            }),
        ) => {
            check(
                client_supported == [ProtocolVersion::V_2026_07_28],
                "expected the modern profile",
            )?;
            check(
                server_supported.len() == 1 && server_supported[0].as_str() == OTHER_MODERN,
                "missing peer version in error",
            )?;
            return Ok(elapsed);
        }
        (Scenario::ModernCapabilityError, Err(ClientInitializeError::JsonRpcError(error)))
            if error.code == ErrorCode::MISSING_REQUIRED_CLIENT_CAPABILITY =>
        {
            return Ok(elapsed);
        }
        (Scenario::ModernNoCommonVersion | Scenario::ModernCapabilityError, Ok(_)) => {
            return Err(io::Error::other(
                "recognized modern error unexpectedly opened a connection",
            ));
        }
        (_, connection) => connection.map_err(io::Error::other)?,
    };

    let version = match scenario {
        Scenario::ModernRetry => MODERN,
        Scenario::OlderLegacyReply => OLDER,
        _ => LEGACY,
    };
    let peer = client
        .peer_info()
        .ok_or_else(|| io::Error::other("missing negotiated peer info"))?;
    check(
        peer.protocol_version.as_str() == version,
        "unexpected negotiated revision",
    )?;
    if matches!(scenario, Scenario::LegacySilence) {
        check(
            elapsed >= Duration::from_millis(9_900),
            "fallback occurred before the 10-second probe timeout",
        )?;
    }
    if !matches!(scenario, Scenario::OlderLegacyReply) {
        let tools = client.list_tools(None).await.map_err(io::Error::other)?;
        check(
            tools.tools.len() == 1 && tools.tools[0].name == "fixture_tool",
            "fixture tool list missing",
        )?;
    }
    let closed = client
        .close_with_timeout(SHUTDOWN_LIMIT)
        .await
        .map_err(io::Error::other)?;
    check(closed.is_some(), "SDK connection did not close")?;
    Ok(elapsed)
}

// The peer exchanges raw JSON-RPC and never uses SDK message or lifecycle types.
fn raw_peer(scenario: Scenario, mut input: impl BufRead, mut output: impl Write) -> io::Result<()> {
    let first = read_frame(&mut input)?;
    let requested = if matches!(scenario, Scenario::ModernRetry) {
        OTHER_MODERN
    } else {
        MODERN
    };
    check_discover(&first, requested)?;

    match scenario {
        Scenario::ModernRetry => {
            send_version_error(&mut output, &first, MODERN)?;
            let retry = read_frame(&mut input)?;
            check_discover(&retry, MODERN)?;
            check(retry["id"] != first["id"], "retry reused the probe ID")?;
            send_frame(
                &mut output,
                json!({"jsonrpc":"2.0", "id":retry["id"], "result":{
                    "resultType":"complete", "supportedVersions":[MODERN], "capabilities":{"tools":{}},
                    "ttlMs":0, "cacheScope":"private"
                }}),
            )?;
        }
        Scenario::ModernNoCommonVersion => {
            send_version_error(&mut output, &first, OTHER_MODERN)?;
            return expect_eof(&mut input);
        }
        Scenario::ModernCapabilityError => {
            send_frame(
                &mut output,
                json!({"jsonrpc":"2.0", "id":first["id"], "error":{
                    "code":-32021, "message":"Missing required client capability"
                }}),
            )?;
            return expect_eof(&mut input);
        }
        Scenario::LegacyError | Scenario::LegacySilence | Scenario::OlderLegacyReply => {
            if !matches!(scenario, Scenario::LegacySilence) {
                send_frame(
                    &mut output,
                    json!({"jsonrpc":"2.0", "id":first["id"], "error":{
                        "code":-32601, "message":"Method not found"
                    }}),
                )?;
            }
            let initialize = read_frame(&mut input)?;
            check_request(&initialize, "initialize")?;
            check(
                initialize["id"] != first["id"],
                "initialize reused the probe ID",
            )?;
            check(
                initialize["params"]["protocolVersion"] == LEGACY,
                "legacy revision offer",
            )?;
            let revision = if matches!(scenario, Scenario::OlderLegacyReply) {
                OLDER
            } else {
                LEGACY
            };
            send_frame(
                &mut output,
                json!({"jsonrpc":"2.0", "id":initialize["id"], "result":{
                    "protocolVersion":revision, "capabilities":{"tools":{}}, "serverInfo":{"name":"fixture", "version":"1"}
                }}),
            )?;
            let initialized = read_frame(&mut input)?;
            check(
                initialized["method"] == "notifications/initialized"
                    && initialized.get("id").is_none(),
                "legacy initialized notification",
            )?;
            if matches!(scenario, Scenario::OlderLegacyReply) {
                return expect_eof(&mut input);
            }
        }
    }

    let list = read_frame(&mut input)?;
    check_request(&list, "tools/list")?;
    let modern = matches!(scenario, Scenario::ModernRetry);
    let metadata = list.pointer("/params/_meta/io.modelcontextprotocol~1protocolVersion");
    check(
        if modern {
            metadata == Some(&json!(MODERN))
        } else {
            metadata.is_none()
        },
        "wrong request metadata after negotiation",
    )?;
    let mut result = json!({"tools":[{"name":"fixture_tool", "inputSchema":{"type":"object"}}]});
    if modern {
        result["resultType"] = json!("complete");
        result["ttlMs"] = json!(0);
        result["cacheScope"] = json!("private");
    }
    send_frame(
        &mut output,
        json!({"jsonrpc":"2.0", "id":list["id"], "result":result}),
    )?;
    expect_eof(&mut input)
}

fn read_frame(input: &mut impl BufRead) -> io::Result<Value> {
    let mut bytes = Vec::new();
    input.take(4096).read_until(b'\n', &mut bytes)?;
    check(bytes.ends_with(b"\n"), "expected a bounded JSON-RPC frame")?;
    let frame: Value = serde_json::from_slice(&bytes)?;
    check(frame["jsonrpc"] == "2.0", "expected JSON-RPC 2.0")?;
    Ok(frame)
}

fn send_frame(output: &mut impl Write, frame: Value) -> io::Result<()> {
    serde_json::to_writer(&mut *output, &frame)?;
    output.write_all(b"\n")?;
    output.flush()
}

fn check_request(frame: &Value, method: &str) -> io::Result<()> {
    check(
        frame["method"] == method && (frame["id"].is_i64() || frame["id"].is_string()),
        method,
    )
}

fn check_discover(frame: &Value, version: &str) -> io::Result<()> {
    check_request(frame, "server/discover")?;
    let metadata = &frame["params"]["_meta"];
    check(
        metadata["io.modelcontextprotocol/protocolVersion"] == version
            && metadata["io.modelcontextprotocol/clientCapabilities"].is_object(),
        "modern discovery metadata",
    )
}

fn send_version_error(output: &mut impl Write, probe: &Value, supported: &str) -> io::Result<()> {
    send_frame(
        output,
        json!({"jsonrpc":"2.0", "id":probe["id"], "error":{
            "code":-32022, "message":"Unsupported protocol version", "data":{"supported":[supported]}
        }}),
    )
}

fn expect_eof(input: &mut impl BufRead) -> io::Result<()> {
    check(input.fill_buf()?.is_empty(), "unexpected frame before EOF")
}

fn check(condition: bool, message: &str) -> io::Result<()> {
    if condition {
        Ok(())
    } else {
        Err(io::Error::other(message))
    }
}
