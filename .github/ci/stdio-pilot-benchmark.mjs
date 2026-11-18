import assert from "node:assert/strict";
import { execFileSync, spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { once } from "node:events";
import { createServer } from "node:http";
import { mkdir, mkdtemp, readFile, realpath, rm, writeFile } from "node:fs/promises";
import { cpus, release, tmpdir, totalmem } from "node:os";
import { dirname, join, resolve } from "node:path";
import { createInterface } from "node:readline";
import { setTimeout as delay } from "node:timers/promises";

const [cliPath, reportPath, prebuiltPath] = process.argv.slice(2);
assert.ok(cliPath && reportPath, "Usage: node stdio-pilot-benchmark.mjs <release-cli> <report.json> [<prebuilt-release-server>]");
assert.equal(process.platform, "win32", "This benchmark measures the Windows pilot working set.");
const cli = await realpath(resolve(cliPath));
const prebuilt = prebuiltPath ? await realpath(resolve(prebuiltPath)) : undefined;
const temporaryParent = await realpath(tmpdir());
const scratch = await realpath(await mkdtemp(join(temporaryParent, "mcp-stdio-benchmark-")));
assert.equal(dirname(scratch), temporaryParent);
const cwd = join(scratch, "project");
const exportDir = join(scratch, "standalone");
const target = resolve(process.env.CARGO_TARGET_DIR ?? join(scratch, "target"));
const manifest = join(exportDir, "mcp-project.json");
const config = join(scratch, "runtime-config.json");
const triple = "x86_64-pc-windows-msvc";
const binary = prebuilt ?? join(target, triple, "release", "mcp-standalone-server.exe");
const sample = new URL("../../crates/builder-project/tests/fixtures/catalog-v1.json", import.meta.url);
const modern = "2026-07-28";
const legacy = "2025-11-25";
const item = { id: "42", name: "Sample item" };

const upstream = createServer((request, response) => {
  if (request.method === "GET" && request.url === "/items/42") {
    response.writeHead(200, { "content-type": "application/json" }).end(JSON.stringify(item));
  } else {
    response.writeHead(404).end();
  }
});
upstream.listen(0, "127.0.0.1");

const sha256 = (data) => "sha256:" + createHash("sha256").update(data).digest("hex");
const p95 = (samples) =>
  samples.length ? samples.toSorted((a, b) => a - b)[Math.ceil(0.95 * samples.length) - 1] : null;

async function deadline(result, label) {
  const controller = new AbortController();
  try {
    return await Promise.race([
      result,
      delay(8_000, undefined, { signal: controller.signal }).then(() => {
        throw new Error(`Timed out waiting for ${label}.`);
      }),
    ]);
  } finally {
    controller.abort();
  }
}

async function measure(revision) {
  const startMs = [];
  const callMs = [];
  const stopMs = [];
  const failures = [];
  let idleWorkingSetBytes = null;
  for (let attempt = 0; attempt < 30; attempt++) {
    const launched = performance.now();
    const child = spawn(binary, ["--manifest", manifest, "--config", config], {
      cwd: exportDir, stdio: ["pipe", "pipe", "inherit"], windowsHide: true,
    });
    // Record spawn failure separately so cleanup can await the close event.
    let spawnError;
    child.on("error", (error) => { spawnError = error; });
    const closed = new Promise((done) => child.once("close", done));
    const lines = createInterface({ input: child.stdout, crlfDelay: Infinity });
    const replies = lines[Symbol.asyncIterator]();
    child.stdin.on("error", () => lines.close());
    let nextId = 0;
    const send = (frame) => child.stdin.write(JSON.stringify({ jsonrpc: "2.0", ...frame }) + "\n");
    async function request(method, params = {}) {
      const id = ++nextId;
      if (revision === modern) {
        params._meta = {
          "io.modelcontextprotocol/protocolVersion": modern,
          "io.modelcontextprotocol/clientCapabilities": {},
        };
      }
      send({ id, method, params });
      const { value, done } = await deadline(replies.next(), method);
      if (spawnError) throw spawnError;
      assert.ok(!done, `The server closed before replying to ${method}.`);
      const frame = JSON.parse(value);
      assert.equal(frame.id, id);
      assert.ok(frame.result, `Expected ${method} to succeed.`);
      return frame.result;
    }
    try {
      if (revision === modern) {
        const discovery = await request("server/discover");
        assert.equal(discovery.supportedVersions[0], modern);
      } else {
        const initialized = await request("initialize", {
          protocolVersion: legacy,
          capabilities: {},
          clientInfo: { name: "stdio-pilot-benchmark", version: "1" },
        });
        assert.equal(initialized.protocolVersion, legacy);
        send({ method: "notifications/initialized" });
        assert.equal((await request("tools/list")).tools[0].name, "lookup_item");
      }
      startMs.push(performance.now() - launched);

      if (attempt === 0) {
        for (let call = 0; call < 220; call++) {
          const sent = performance.now();
          const result = await request("tools/call", { name: "lookup_item", arguments: { id: "42" } });
          assert.deepEqual(result.structuredContent, item);
          if (call >= 20) callMs.push(performance.now() - sent);
        }
        await delay(60_000);
        assert.ok(Number.isSafeInteger(child.pid) && child.pid > 0);
        idleWorkingSetBytes = Number(execFileSync("powershell.exe", [
          "-NoLogo", "-NoProfile", "-NonInteractive", "-Command",
          `(Get-Process -Id ${child.pid} -ErrorAction Stop).WorkingSet64`,
        ], { encoding: "utf8", timeout: 8_000, windowsHide: true }).trim());
        assert.ok(Number.isSafeInteger(idleWorkingSetBytes) && idleWorkingSetBytes > 0);
      }
      const stopping = performance.now();
      child.stdin.end();
      assert.equal(await deadline(closed, "standalone process exit"), 0);
      if (attempt < 10) stopMs.push(performance.now() - stopping);
    } catch (error) {
      failures.push({ attempt: attempt + 1, message: error.message });
      if (child.exitCode === null && child.signalCode === null) child.kill();
      await deadline(closed, "standalone process cleanup");
    } finally {
      lines.close();
    }
  }
  return {
    revision, startMs, callMs, stopMs, idleWorkingSetBytes, failures,
    p95: { startMs: p95(startMs), callMs: p95(callMs), stopMs: p95(stopMs) },
  };
}

try {
  await once(upstream, "listening");
  const fixture = await readFile(sample);
  const project = JSON.parse(fixture);
  const origin = `http://127.0.0.1:${upstream.address().port}`;
  project.bindings[0].origin = origin;
  delete project.bindings[0].headersFromSecrets;
  project.secretRefs = [];
  Object.assign(project.policy.grants[0].target, {
    origin, pathTemplate: "/items/42", allowedPeerRanges: ["127.0.0.1/32"],
  });
  await mkdir(cwd);
  await writeFile(join(cwd, "mcp-project.json"), JSON.stringify(project));
  execFileSync(cli, ["generate", cwd, "--out", exportDir], { stdio: "inherit", windowsHide: true });
  if (!prebuilt) {
    execFileSync("cargo", ["build", "--release", "--locked", "--target", triple, "--target-dir", target], {
      cwd: exportDir, stdio: "inherit", windowsHide: true,
    });
  }
  const manifestRevision = sha256(await readFile(manifest));
  await writeFile(config, JSON.stringify({
    configVersion: 1, kind: "stdio", manifestRevision, approvedGrantIds: ["catalog_read"],
  }));
  const report = {
    commit: execFileSync("git", ["rev-parse", "HEAD"], { encoding: "utf8" }).trim(),
    platform: release(), arch: process.arch, cpu: cpus()[0].model, logicalCpus: cpus().length,
    memoryBytes: totalmem(), fixtureDigest: sha256(fixture), manifestRevision,
    cliDigest: sha256(await readFile(cli)),
    serverDigest: sha256(await readFile(binary)),
    serverBuild: prebuilt ? "prebuilt" : "builtForRun",
    transport: "stdio",
    profiles: [await measure(modern), await measure(legacy)],
  };
  await writeFile(resolve(reportPath), JSON.stringify(report, null, 2) + "\n", { flag: "wx" });
  for (const { revision, p95, idleWorkingSetBytes, failures } of report.profiles) {
    console.log({ revision, p95, idleWorkingSetBytes, failures: failures.length });
  }
  assert.ok(report.profiles.every(({ failures }) => !failures.length));
} finally {
  upstream.closeAllConnections();
  await new Promise((done) => upstream.close(done));
  await rm(scratch, { recursive: true, force: true });
}
