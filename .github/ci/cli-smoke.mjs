import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const fixture = JSON.parse(
  readFileSync(new URL("../../crates/builder-app/tests/fixtures/workspace-smoke.json", import.meta.url), "utf8"),
);
const cli = resolve(
  process.env.CARGO_TARGET_DIR ?? "target",
  "debug",
  process.platform === "win32" ? "mcp-builder.exe" : "mcp-builder",
);
const uuidV4 = /^[\da-f]{8}-[\da-f]{4}-4[\da-f]{3}-[89ab][\da-f]{3}-[\da-f]{12}$/i;

function run(args, status, log = "off") {
  const result = spawnSync(cli, args, {
    encoding: "utf8",
    env: { ...process.env, RUST_LOG: log },
    timeout: 15_000,
    windowsHide: true,
  });
  if (result.error) throw result.error;
  assert.equal(result.status, status, `${args.join(" ")}: ${result.stderr}`);
  return result;
}

function checkJson(args, status, expected, traceMessage) {
  const { stdout, stderr } = run(args, status, "builder_app=info");
  const actual = JSON.parse(stdout);
  const payload = expected.ok ? actual.data : actual.error;
  const correlationId = payload.correlationId;
  assert.match(correlationId, uuidV4);
  payload.correlationId = (expected.ok ? expected.data : expected.error).correlationId;
  assert.deepEqual(actual, expected);
  if (traceMessage) {
    const trace = stderr.trim().split("\n").map((line) => JSON.parse(line));
    assert.ok(trace.some(({ target, fields }) =>
      target === "builder_app::smoke" &&
      fields.message === traceMessage &&
      fields.correlation_id === correlationId
    ), "stderr trace must match the response correlation ID");
  } else {
    assert.equal(stderr, "");
  }
}

const { builderVersion, contractVersion } = fixture.supported.reply.data;
const invalidVersion = String(fixture.unsupported.request.contractVersion);
const diagnostic = fixture.unsupported.reply.error;
assert.match(run(["--help"], 0).stdout, /^\s+smoke\s+/m);
assert.equal(run(["--version"], 0).stdout.trim(), `mcp-builder ${builderVersion}`);

const success = run(["smoke"], 0);
assert.equal(success.stdout.trim(), `Workspace smoke passed (builder ${builderVersion}, contract ${contractVersion}).`);
assert.equal(success.stderr, "");
const rejection = run(["smoke", "--contract-version", invalidVersion], 2);
assert.equal(rejection.stdout, "");
assert.deepEqual(rejection.stderr.trim().split(/\r?\n/), [diagnostic.message, diagnostic.action]);

checkJson(["smoke", "--json"], 0, { version: 1, command: "smoke", ...fixture.supported.reply }, "Workspace smoke completed");
checkJson(
  ["--json", "smoke", "--contract-version", invalidVersion],
  2,
  { version: 1, command: "smoke", ...fixture.unsupported.reply },
  "Workspace smoke request rejected",
);
checkJson(["smoke", "--json", "--unused-fixture-flag"], 2, {
  version: 1,
  command: "cli",
  ok: false,
  error: {
    code: "cli.usage",
    message: "Invalid command or options.",
    severity: "error",
    correlationId: diagnostic.correlationId,
    action: "Run mcp-builder --help for usage.",
  },
  diagnostics: [],
});

console.log("CLI help, version, text and versioned JSON passed with expected stdout, stderr and exits 0/2.");
