import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { createHash } from "node:crypto";
import { once } from "node:events";
import { mkdir, mkdtemp, readFile, realpath, rm, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { promisify } from "node:util";

const [launcherPath, serverPath, builderPath] = process.argv.slice(2);
assert.ok(launcherPath && serverPath, "Usage: node standalone-inspector-smoke.mjs <Inspector launcher.js> <standalone executable> [<builder CLI>]");
const launcher = await realpath(resolve(launcherPath));
const standalone = await realpath(resolve(serverPath));
const parent = await realpath(tmpdir());
const scratch = await realpath(await mkdtemp(join(parent, "mcp-standalone-inspector-")));
assert.equal(dirname(scratch), parent);
const exec = promisify(execFile);
const item = { id: "42", name: "Sample item" };
const calls = [];
const upstream = createServer((request, response) => {
  calls.push([request.method, request.url]);
  if (request.method === "GET" && request.url === "/items/42") {
    response.writeHead(200, { "content-type": "application/json" }).end(JSON.stringify(item));
  } else {
    response.writeHead(404).end();
  }
});

try {
  upstream.listen(0, "127.0.0.1");
  await once(upstream, "listening");
  const origin = `http://127.0.0.1:${upstream.address().port}`;
  const documents = join(scratch, "fixture files");
  const storage = join(scratch, "inspector state");
  await mkdir(documents);
  await mkdir(join(documents, "notes"));
  await mkdir(storage);
  await writeFile(join(documents, "guide.txt"), "Catalog guide");
  await writeFile(join(documents, "notes", "42.txt"), "Catalog note");

  const project = JSON.parse(await readFile(new URL(
    "../../crates/builder-project/tests/fixtures/catalog-v1.json", import.meta.url,
  ), "utf8"));
  project.bindings[0].origin = origin;
  delete project.bindings[0].headersFromSecrets;
  project.secretRefs = [];
  Object.assign(project.policy.grants[0].target, {
    origin, pathTemplate: "/items/42", allowedPeerRanges: ["127.0.0.1/32"],
  });
  project.policy.grants.push({
    id: "guide_read", capabilityId: "catalog_guide", operation: "file.read",
    target: { kind: "fileRead", rootRef: "catalog_docs", relativePathTemplate: "guide.txt" },
  });
  const manifest = join(scratch, "mcp-project.json");
  const runtime = join(scratch, "runtime-config.json");
  const config = join(scratch, "inspector.json");
  const manifestBytes = JSON.stringify(project);
  await writeFile(manifest, manifestBytes);
  await writeFile(runtime, JSON.stringify({
    configVersion: 1,
    kind: "stdio",
    manifestRevision: "sha256:" + createHash("sha256").update(manifestBytes).digest("hex"),
    approvedGrantIds: ["catalog_read", "guide_read", "note_read"],
    approvedPromptIds: ["review_item"],
    targetBindings: { catalog_docs: { kind: "fileRead", path: documents } },
  }));
  if (builderPath) {
    const builder = await realpath(resolve(builderPath));
    await exec(builder, [
      "client", "export", "generic", "--name", "catalog", "--executable", standalone,
      "--manifest", manifest, "--runtime-config", runtime, "--out", config,
    ], { cwd: scratch, timeout: 15_000, windowsHide: true });
  } else {
    await writeFile(config, JSON.stringify({
      mcpServers: {
        catalog: { type: "stdio", command: standalone, cwd: scratch, args: ["--manifest", manifest, "--config", runtime] },
      },
    }));
  }
  const env = {
    ...process.env,
    MCP_STORAGE_DIR: storage,
    MCP_INSPECTOR_SECRET_STORE: "memory",
    MCP_INSPECTOR_PERSIST_TOKENS: "none",
    MCP_AUTO_OPEN_ENABLED: "false",
  };
  delete env.MCP_CATALOG_PATH;
  const inspect = (...args) => exec(process.execPath, [launcher, "--cli", ...args], {
    cwd: scratch, encoding: "utf8", env, timeout: 20_000, maxBuffer: 1 << 20, windowsHide: true,
  });
  const { stdout: version } = await inspect("--version");
  assert.equal(version.trim(), "2.15.0");
  console.log(`Inspector ${version.trim()} on Node ${process.version} (${process.platform} ${process.arch})`);
  for (const [era, revision] of [["modern", "2026-07-28"], ["legacy", "2025-11-25"]]) {
    const flags = ["--config", config, "--server", "catalog", "--protocol-era", era, "--format", "json", "--quiet"];
    const invoke = async (method, ...args) => {
      const { stdout } = await inspect(...flags, "--method", method, ...args);
      const reply = JSON.parse(stdout);
      assert.ok(reply.result, `Expected ${era} ${method} to succeed`);
      return reply.result;
    };
    const hello = await invoke("initialize");
    assert.equal(hello.protocolVersion, revision);
    assert.equal(hello.serverInfo.name, "catalog");
    assert.equal((await invoke("tools/list")).tools[0].name, "lookup_item");
    const result = await invoke("tools/call", "--tool-name", "lookup_item", "--tool-args-json", '{"id":"42"}');
    assert.deepEqual(result.structuredContent, item);
    assert.equal((await invoke("resources/list")).resources[0].uri, "catalog://docs/guide");
    const templates = await invoke("resources/templates/list");
    assert.equal(templates.resourceTemplates[0].uriTemplate, "catalog://docs/items/{id}");
    for (const [uri, expected] of [["catalog://docs/guide", "Catalog guide"], ["catalog://docs/items/42", "Catalog note"]]) {
      const result = await invoke("resources/read", "--uri", uri);
      assert.deepEqual(result.contents, [{ uri, mimeType: "text/plain", text: expected }]);
    }
    assert.equal((await invoke("prompts/list")).prompts[0].name, "review_item");
    const prompt = await invoke("prompts/get", "--prompt-name", "review_item", "--prompt-args", "id=42");
    assert.deepEqual(prompt.messages, [{ role: "user", content: { type: "text", text: "Review catalog item 42." } }]);
    console.log(`${era} ${revision}: negotiation, tools, fixed and templated resources, and prompts passed`);
  }
  assert.deepEqual(calls, [["GET", "/items/42"], ["GET", "/items/42"]]);
  console.log("Observed exactly two approved loopback requests");
} finally {
  upstream.closeAllConnections();
  if (upstream.listening) await new Promise((done) => upstream.close(done));
  await rm(scratch, { recursive: true, force: true });
}
