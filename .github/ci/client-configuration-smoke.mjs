import assert from "node:assert/strict";
import { execFileSync, spawn, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { chmodSync, copyFileSync, readFileSync, realpathSync, writeFileSync } from "node:fs";
import { isAbsolute, join } from "node:path";
import { createInterface } from "node:readline";

const modern = "2026-07-28";
const legacy = "2025-11-25";
const name = "catalog_ci";
const secretName = "MCP_CLIENT_EXPORT_SMOKE_SECRET";
const secretValue = "mcp-client-export-smoke-private-value";
const extensions = {
  "claude-code": "claude-code.json",
  codex: "codex.toml",
  opencode: "opencode.json",
  generic: "mcp-client.json",
};

function launchFields(profile, contents) {
  if (profile === "codex") {
    const parsed = JSON.parse(execFileSync(process.platform === "win32" ? "python" : "python3", [
      "-c",
      "import json, sys, tomllib\nprint(json.dumps(tomllib.loads(sys.stdin.buffer.read().decode('utf-8'))))",
    ], { encoding: "utf8", input: contents, windowsHide: true, timeout: 15_000 }));
    const entry = parsed.mcp_servers[name];
    assert.deepEqual(Object.keys(entry).sort(), ["args", "command"]);
    return [entry.command, entry.args];
  }
  const parsed = JSON.parse(contents);
  if (profile === "opencode") {
    const entry = parsed.mcp.servers[name];
    assert.equal(entry.type, "local");
    assert.ok(Array.isArray(entry.command));
    assert.ok(!Object.hasOwn(entry, "environment"));
    return [entry.command[0], entry.command.slice(1)];
  }
  const entry = parsed.mcpServers[name];
  assert.equal(entry.type, "stdio");
  assert.deepEqual(Object.keys(entry).sort(), ["args", "command", "type"]);
  return [entry.command, entry.args];
}

function assertSameAbsolutePath(actual, expected) {
  assert.equal(typeof actual, "string");
  assert.ok(isAbsolute(actual), "Expected an absolute native path");
  assert.equal(realpathSync(actual), realpathSync(expected));
}

async function exerciseStdio(command, args, cwd, revision, approved) {
  const child = spawn(command, args, {
    cwd, stdio: ["pipe", "pipe", "inherit"], windowsHide: true,
  });
  let processError;
  child.on("error", (error) => { processError = error; });
  const closed = new Promise((done) => child.once("close", done));
  const lines = createInterface({ input: child.stdout, crlfDelay: Infinity });
  const replies = lines[Symbol.asyncIterator]();
  child.stdin.on("error", (error) => {
    processError = error;
    lines.close();
  });
  const timeout = setTimeout(() => {
    processError = new Error("Client profile stdio check timed out");
    child.kill("SIGKILL");
    lines.close();
  }, 15_000);
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
    const { value, done } = await replies.next();
    if (processError) throw processError;
    assert.ok(!done, "Standalone server closed before replying to " + method);
    const frame = JSON.parse(value);
    assert.equal(frame.jsonrpc, "2.0");
    assert.equal(frame.id, id);
    assert.ok(frame.result, method + ": " + value);
    return frame.result;
  }
  try {
    if (revision === modern) {
      const discovery = await request("server/discover");
      assert.equal(discovery._meta["io.modelcontextprotocol/serverInfo"].name, "catalog");
      assert.deepEqual(discovery.supportedVersions, [modern, legacy]);
    } else {
      const initialized = await request("initialize", {
        protocolVersion: legacy,
        capabilities: {},
        clientInfo: { name: "client-export-smoke", version: "1" },
      });
      assert.equal(initialized.protocolVersion, legacy);
      assert.equal(initialized.serverInfo.name, "catalog");
      send({ method: "notifications/initialized" });
    }
    const prompts = (await request("prompts/list")).prompts;
    assert.deepEqual(prompts.map((prompt) => prompt.name), approved ? ["review_item"] : []);
    if (approved) {
      const prompt = await request("prompts/get", { name: "review_item", arguments: { id: "42" } });
      assert.deepEqual(prompt.messages, [
        { role: "user", content: { type: "text", text: "Review catalog item 42." } },
      ]);
    }
    child.stdin.end();
    assert.equal(await closed, 0, "Standalone server must exit successfully at EOF");
    if (processError) throw processError;
  } finally {
    clearTimeout(timeout);
    lines.close();
    if (child.exitCode === null && child.signalCode === null) child.kill("SIGKILL");
    await closed;
  }
}

export async function checkClientConfigurations({ builder, standalone, output, scratch }) {
  const suffix = process.platform === "win32" ? ".exe" : "";
  const executable = join(output, "catalog server café" + suffix);
  copyFileSync(standalone, executable);
  if (process.platform !== "win32") chmodSync(executable, 0o700);
  const manifest = join(output, "mcp-project.json");
  const runtime = join(output, "runtime config café.json");
  writeFileSync(runtime, JSON.stringify({
    configVersion: 1,
    kind: "stdio",
    manifestRevision: "sha256:" + createHash("sha256").update(readFileSync(manifest)).digest("hex"),
    approvedPromptIds: ["review_item"],
    secretSources: { catalog_auth: { kind: "env", name: secretName } },
  }), { flag: "wx" });
  function exportClient(profile, options) {
    const result = spawnSync(builder, [
      "client", "export", profile, "--name", name, "--executable", executable,
      "--manifest", manifest, ...options,
    ], {
      cwd: scratch, encoding: "utf8", timeout: 15_000, windowsHide: true,
      env: { ...process.env, [secretName]: secretValue },
    });
    if (result.error) throw result.error;
    assert.equal(result.signal, null);
    return result;
  }
  for (const [profile, extension] of Object.entries(extensions)) {
    const fileName = "mcp-" + name + "." + extension;
    const file = join(scratch, fileName);
    const args = ["--runtime-config", runtime, "--out", file, "--json"];
    const result = exportClient(profile, args);
    assert.equal(result.status, 0, result.stderr || result.stdout);
    const { version, command, ok, data } = JSON.parse(result.stdout);
    assert.equal(version, 1);
    assert.equal(command, "client.export");
    assert.equal(ok, true);
    assert.equal(data.target, profile);
    assert.equal(data.suggestedFileName, fileName);
    assertSameAbsolutePath(data.destination, file);
    const contents = readFileSync(file, "utf8");
    assert.equal(data.contents, contents);
    for (const privateText of [secretName, secretValue, "secretSources", "approvedPromptIds"]) {
      assert.ok(!contents.includes(privateText), "Runtime configuration contents must stay private");
    }
    const [program, argv] = launchFields(profile, contents);
    assertSameAbsolutePath(program, executable);
    assert.equal(argv.length, 4);
    assert.deepEqual([argv[0], argv[2]], ["--manifest", "--config"]);
    assertSameAbsolutePath(argv[1], manifest);
    assertSameAbsolutePath(argv[3], runtime);
    const duplicate = exportClient(profile, args);
    assert.equal(duplicate.status, 6, duplicate.stderr || duplicate.stdout);
    assert.equal(JSON.parse(duplicate.stdout).error.code, "export.destination_exists");
    assert.equal(readFileSync(file, "utf8"), contents);
    for (const revision of [modern, legacy]) {
      await exerciseStdio(program, argv, scratch, revision, true);
    }
  }
  const defaultProfile = exportClient("generic", []);
  assert.equal(defaultProfile.status, 0, defaultProfile.stderr);
  const [program, argv] = launchFields("generic", defaultProfile.stdout);
  assertSameAbsolutePath(program, executable);
  assert.equal(argv.length, 2);
  assert.equal(argv[0], "--manifest");
  assertSameAbsolutePath(argv[1], manifest);
  await exerciseStdio(program, argv, scratch, modern, false);
  console.log("All four client configuration formats preserved native paths and launched both MCP revisions.");
  console.log("The runtime configuration stayed private, existing output survived and default approvals stayed empty.");
}
