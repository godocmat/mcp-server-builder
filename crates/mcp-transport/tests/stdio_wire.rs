#[path = "stdio_wire/fixture.rs"]
mod fixture;

use std::io;
use std::process::Stdio;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

const MODERN: &str = "2026-07-28";
const LEGACY: &str = "2025-11-25";
const WAIT: Duration = Duration::from_secs(6);
const FRAME_LIMIT: usize = 1024 * 1024;
const STDERR_CANARY: &str = "stdio-synthetic-cancellation-canary";
const DIAGNOSTIC: &str = "mcp-server-builder/diagnostic";

#[tokio::main(flavor = "current_thread")]
async fn main() -> io::Result<()> {
    if std::env::args().nth(1).as_deref() == Some("--stdio-fixture") {
        return fixture::run().await;
    }
    let mut passed = true;
    for (name, scenario) in [
        ("modern capabilities", Scenario::Capabilities(true)),
        ("legacy capabilities", Scenario::Capabilities(false)),
        ("framing and metadata", Scenario::Framing),
        ("modern cancellation", Scenario::Cancellation(true)),
        ("legacy cancellation", Scenario::Cancellation(false)),
        ("bounded EOF", Scenario::Eof),
    ] {
        let mut connection = Connection::start()?;
        let outcome = tokio::time::timeout(WAIT, async {
            match scenario {
                Scenario::Capabilities(modern) => capabilities(&mut connection, modern).await,
                Scenario::Framing => framing(&mut connection).await,
                Scenario::Cancellation(modern) => cancellation(&mut connection, modern).await,
                Scenario::Eof => eof_while_active(&mut connection).await,
            }
        })
        .await;
        let result = match outcome {
            Ok(Ok(())) => connection.finish().await,
            Ok(Err(error)) => Err(error),
            Err(_) => Err(io::Error::other("wire scenario timed out")),
        };
        if let Err(error) = result {
            connection.reap().await;
            eprintln!("failed {name}: {error}");
            passed = false;
        } else {
            println!("passed {name}");
        }
    }
    check(passed, "stdio conformance failed")
}

#[derive(Clone, Copy)]
enum Scenario {
    Capabilities(bool),
    Framing,
    Cancellation(bool),
    Eof,
}

struct Connection {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    stderr: tokio::task::JoinHandle<io::Result<Vec<u8>>>,
}

impl Connection {
    fn start() -> io::Result<Self> {
        let mut child = Command::new(std::env::current_exe()?)
            .arg("--stdio-fixture")
            .env("RUST_LOG", "rmcp=trace")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let stderr = child.stderr.take().expect("fixture stderr");
        Ok(Self {
            stdin: child.stdin.take(),
            stdout: BufReader::new(child.stdout.take().expect("fixture stdout")),
            child,
            stderr: tokio::spawn(async move {
                let mut log = Vec::new();
                stderr
                    .take(FRAME_LIMIT as u64)
                    .read_to_end(&mut log)
                    .await?;
                Ok(log)
            }),
        })
    }

    async fn raw(&mut self, bytes: &[u8]) -> io::Result<()> {
        let writer = self.stdin.as_mut().expect("fixture stdin");
        writer.write_all(bytes).await?;
        writer.flush().await
    }

    async fn send(&mut self, message: Value) -> io::Result<()> {
        let mut bytes = serde_json::to_vec(&message)?;
        bytes.push(b'\n');
        self.raw(&bytes).await
    }

    async fn read(&mut self) -> io::Result<Value> {
        let mut line = Vec::new();
        (&mut self.stdout)
            .take((FRAME_LIMIT + 1) as u64)
            .read_until(b'\n', &mut line)
            .await?;
        check(
            line.len() <= FRAME_LIMIT && line.ends_with(b"\n"),
            "invalid stdout frame",
        )?;
        let value: Value = serde_json::from_slice(&line)?;
        check(value["jsonrpc"] == "2.0", "stdout must carry JSON-RPC")?;
        Ok(value)
    }

    async fn response(&mut self, id: impl Into<Value>) -> io::Result<Value> {
        let id = id.into();
        let value = self.read().await?;
        if value.get("id") != Some(&id) {
            return Err(io::Error::other(format!(
                "expected response {id}, got id {} method {} error {}",
                value["id"], value["method"], value["error"]
            )));
        }
        Ok(value)
    }

    async fn exchange(
        &mut self,
        id: i64,
        method: &str,
        params: Value,
        modern: bool,
    ) -> io::Result<Value> {
        self.send(request(id, method, params, modern)).await?;
        self.response(id).await
    }

    async fn close_input(&mut self) -> io::Result<()> {
        if let Some(mut writer) = self.stdin.take() {
            writer.shutdown().await?;
        }
        Ok(())
    }

    async fn finish(&mut self) -> io::Result<()> {
        self.close_input().await?;
        let result = tokio::time::timeout(Duration::from_secs(2), self.child.wait())
            .await
            .map_err(|_| io::Error::other("fixture did not exit within two seconds of EOF"))??;
        check(result.success(), "fixture subprocess exited unsuccessfully")?;
        let log = (&mut self.stderr).await.map_err(io::Error::other)??;
        let log = String::from_utf8_lossy(&log);
        check(
            !log.contains(STDERR_CANARY) && !log.contains("rmcp::"),
            "SDK trace reached stderr",
        )?;
        check(
            self.stdout.fill_buf().await?.is_empty(),
            "unexpected trailing stdout",
        )
    }

    async fn reap(&mut self) {
        let _ = self.child.start_kill();
        let _ = tokio::time::timeout(Duration::from_secs(2), self.child.wait()).await;
        self.stderr.abort();
    }
}

fn request(id: impl Into<Value>, method: &str, mut params: Value, modern: bool) -> Value {
    if modern {
        params["_meta"]["io.modelcontextprotocol/protocolVersion"] = json!(MODERN);
        params["_meta"]["io.modelcontextprotocol/clientCapabilities"] = json!({});
    }
    json!({"jsonrpc":"2.0","id":id.into(),"method":method,"params":params})
}

fn call(id: impl Into<Value>, item: Value, progress: Option<Value>, modern: bool) -> Value {
    let mut params = json!({"name":"lookup_item", "arguments":{"id":item}});
    if let Some(token) = progress {
        params["_meta"]["progressToken"] = token;
    }
    request(id, "tools/call", params, modern)
}

fn cancel(id: impl Into<Value>) -> Value {
    json!({"jsonrpc":"2.0","method":"notifications/cancelled",
        "params":{"requestId":id.into(),"reason":STDERR_CANARY}})
}

fn check(condition: bool, message: &str) -> io::Result<()> {
    if condition {
        Ok(())
    } else {
        Err(io::Error::other(message.to_owned()))
    }
}

async fn initialize(connection: &mut Connection, version: &str) -> io::Result<()> {
    let response = connection
        .exchange(
            0,
            "initialize",
            json!({
                "protocolVersion":version,
                "capabilities":{},
                "clientInfo":{"name":"raw-fixture","version":"1"}
            }),
            false,
        )
        .await?;
    check(
        response["result"]["protocolVersion"] == LEGACY,
        "legacy negotiation",
    )?;
    check(
        response["result"]["serverInfo"]["name"] == "catalog",
        "legacy identity",
    )?;
    connection
        .send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
        .await
}

async fn capabilities(connection: &mut Connection, modern: bool) -> io::Result<()> {
    if modern {
        let discover = connection
            .exchange(0, "server/discover", json!({}), true)
            .await?;
        let result = &discover["result"];
        check(
            result["supportedVersions"] == json!([MODERN, LEGACY]),
            "discovery versions",
        )?;
        check(
            result["_meta"]["io.modelcontextprotocol/serverInfo"]["name"] == "catalog",
            "discovery identity",
        )?;
        check(
            result["capabilities"] == json!({"tools":{},"resources":{},"prompts":{}}),
            "discovery capabilities",
        )?;
        cache(result, true)?;
    } else {
        initialize(connection, LEGACY).await?;
        let ping = connection.exchange(11, "ping", json!({}), false).await?;
        check(ping["result"] == json!({}), "legacy ping")?;
    }
    let list = connection
        .exchange(1, "tools/list", json!({}), modern)
        .await?;
    let result = &list["result"];
    check(result["tools"][0]["name"] == "lookup_item", "tool list")?;
    check(
        result["tools"][0]["inputSchema"]["type"] == "object"
            && result["tools"][0]["outputSchema"]["type"] == "object",
        "tool schemas",
    )?;
    cache(result, modern)?;
    connection.send(call(2, json!("42"), None, modern)).await?;
    let result = connection.response(2).await?;
    check(
        result["result"]["structuredContent"]["id"] == "42",
        "structured tool result",
    )?;
    check(
        result["result"]["content"][0]["type"] == "text",
        "tool text result",
    )?;
    connection.send(call(3, json!(42), None, modern)).await?;
    let invalid = connection.response(3).await?;
    check(
        invalid["result"]["isError"] == true
            && diagnostic(&invalid["result"]["_meta"], "input.invalid"),
        "invalid tool input",
    )?;
    connection
        .send(call(4, json!("denied"), None, modern))
        .await?;
    let denied = connection.response(4).await?;
    check(
        denied["result"]["isError"] == true
            && diagnostic(&denied["result"]["_meta"], "policy.denied"),
        "tool policy denial",
    )?;
    let resources = connection
        .exchange(5, "resources/list", json!({}), modern)
        .await?;
    check(
        resources["result"]["resources"][0]["uri"] == "catalog://docs/guide",
        "resource list",
    )?;
    cache(&resources["result"], modern)?;
    let templates = connection
        .exchange(6, "resources/templates/list", json!({}), modern)
        .await?;
    check(
        templates["result"]["resourceTemplates"][0]["uriTemplate"] == "catalog://docs/items/{id}",
        "resource template list",
    )?;
    cache(&templates["result"], modern)?;
    for (id, uri, text) in [
        (7, "catalog://docs/guide", "guide.txt"),
        (8, "catalog://docs/items/42", "notes/42.txt"),
    ] {
        let resource = connection
            .exchange(id, "resources/read", json!({"uri":uri}), modern)
            .await?;
        check(
            resource["result"]["contents"][0]
                == json!({"uri":uri,"mimeType":"text/plain","text":text}),
            "resource read",
        )?;
        cache(&resource["result"], modern)?;
    }
    let missing = connection
        .exchange(
            9,
            "resources/read",
            json!({"uri":"catalog://docs/missing"}),
            modern,
        )
        .await?;
    check(
        missing["error"]["code"] == if modern { -32602 } else { -32002 },
        "resource not found code",
    )?;
    check(
        diagnostic(&missing["error"]["data"], "mcp.unknown_resource"),
        "resource error diagnostic",
    )?;
    let prompts = connection
        .exchange(10, "prompts/list", json!({}), modern)
        .await?;
    check(
        prompts["result"]["prompts"][0]["name"] == "review_item",
        "prompt list",
    )?;
    cache(&prompts["result"], modern)?;
    let prompt = connection
        .exchange(
            12,
            "prompts/get",
            json!({"name":"review_item","arguments":{"id":"42"}}),
            modern,
        )
        .await?;
    check(
        prompt["result"]["messages"][0]
            == json!({"role":"user","content":{"type":"text","text":"Review catalog item 42."}}),
        "prompt rendering",
    )
}

fn cache(result: &Value, modern: bool) -> io::Result<()> {
    let matches = if modern {
        result["resultType"] == "complete"
            && result["ttlMs"] == 0
            && result["cacheScope"] == "private"
    } else {
        ["resultType", "ttlMs", "cacheScope"]
            .into_iter()
            .all(|key| result.get(key).is_none())
    };
    check(matches, "revision-specific cache and result metadata")
}

fn diagnostic(container: &Value, code: &str) -> bool {
    let value = &container[DIAGNOSTIC];
    value["code"] == code
        && value["correlationId"]
            .as_str()
            .is_some_and(|id| id.len() == 36)
        && value.as_object().is_some_and(|fields| fields.len() == 2)
}

async fn framing(connection: &mut Connection) -> io::Result<()> {
    let ping = connection.exchange(1, "ping", json!({}), true).await?;
    check(ping["error"]["code"] == -32601, "pre-discovery modern ping")?;
    let direct = connection
        .exchange(2, "tools/list", json!({}), true)
        .await?;
    check(
        direct["result"]["tools"][0]["name"] == "lookup_item",
        "direct modern request",
    )?;
    let missing = connection
        .exchange(3, "tools/list", json!({}), false)
        .await?;
    check(
        missing["error"]["code"] == -32602,
        "missing modern metadata",
    )?;
    let mut unsupported = request(4, "tools/list", json!({}), true);
    unsupported["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"] = json!("2099-01-01");
    connection.send(unsupported).await?;
    let wrong = connection.response(4).await?;
    check(
        wrong["error"]["code"] == -32022
            && wrong["error"]["data"]["supported"] == json!([MODERN, LEGACY]),
        "unsupported version",
    )?;
    connection.raw(b"{malformed\n").await?;
    let parse = connection.read().await?;
    check(
        parse["error"]["code"] == -32700 && parse.get("id").is_none(),
        "parse error",
    )?;
    connection.raw(b"[]\r\n").await?;
    let shape = connection.read().await?;
    check(
        shape["error"]["code"] == -32600 && shape.get("id").is_none(),
        "invalid message shape",
    )?;
    let missing_name = connection
        .exchange(5, "tools/call", json!({}), true)
        .await?;
    check(
        missing_name["error"]["code"] == -32602,
        "known method with malformed parameters",
    )?;
    let unknown = connection
        .exchange(6, "fixture/unavailable", json!({}), true)
        .await?;
    check(unknown["error"]["code"] == -32601, "unknown method")?;
    let mut exact = request(7, "tools/list", json!({"padding":""}), true);
    exact["params"]["padding"] =
        json!("p".repeat(FRAME_LIMIT - serde_json::to_vec(&exact)?.len() - 1));
    connection.send(exact.clone()).await?;
    check(
        connection.response(7).await?["result"]["tools"].is_array(),
        "exact frame ceiling",
    )?;
    let mut over = serde_json::to_vec(&exact)?;
    over.extend_from_slice(b" \n");
    connection.raw(&over).await?;
    let oversized = connection.read().await?;
    check(
        oversized["error"]["code"] == -32600 && oversized.get("id").is_none(),
        "oversized input frame",
    )?;
    let again = connection
        .exchange(8, "tools/list", json!({}), true)
        .await?;
    check(again["result"]["tools"].is_array(), "after rejected frame")
}

async fn start_slow(connection: &mut Connection, modern: bool) -> io::Result<()> {
    connection
        .send(call("slow", json!("slow"), Some(json!("progress")), modern))
        .await?;
    let progress = connection.read().await?;
    check(
        progress["method"] == "notifications/progress"
            && progress["params"]["progressToken"] == "progress"
            && progress["params"]["progress"] == 1.0
            && progress["params"].get("message").is_none(),
        "request progress",
    )
}

async fn cancellation(connection: &mut Connection, modern: bool) -> io::Result<()> {
    if !modern {
        initialize(connection, "2025-03-26").await?;
    }
    start_slow(connection, modern).await?;
    connection
        .send(call("slow", json!("42"), None, modern))
        .await?;
    let duplicate = connection.read().await?;
    check(
        duplicate["error"]["code"] == -32600 && duplicate.get("id").is_none(),
        "duplicate ID isolation",
    )?;
    connection
        .send(call(12, json!("42"), Some(json!("progress")), modern))
        .await?;
    check(
        connection.response(12).await?["error"]["code"] == -32602,
        "duplicate active progress token",
    )?;
    connection.send(cancel("unrelated")).await?;
    connection
        .send(call(13, json!("slow"), Some(json!(13)), modern))
        .await?;
    let progress = connection.read().await?;
    check(
        progress["params"]["progressToken"] == 13,
        "sibling progress token",
    )?;
    connection.send(cancel("slow")).await?;
    connection
        .send(call("slow", json!("42"), Some(json!("progress")), modern))
        .await?;
    let reused = connection.response("slow").await?;
    check(
        reused["result"]["structuredContent"]["id"] == "42",
        "reused ID and token after cancellation",
    )?;
    let timed_out = connection.response(13).await?;
    check(
        timed_out["result"]["isError"] == true
            && diagnostic(&timed_out["result"]["_meta"], "runtime.timeout"),
        "sibling reached its own deadline",
    )?;
    connection.send(cancel("slow")).await?;
    let after = connection
        .exchange(14, "tools/list", json!({}), modern)
        .await?;
    check(
        after["result"]["tools"].is_array(),
        "connection survives cancellation",
    )
}

async fn eof_while_active(connection: &mut Connection) -> io::Result<()> {
    start_slow(connection, true).await
}
