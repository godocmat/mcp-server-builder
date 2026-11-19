import assert from "node:assert/strict";
import { execFile, spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { once } from "node:events";
import { createReadStream } from "node:fs";
import { chmod, copyFile, mkdir, mkdtemp, readFile, readdir, realpath, rm, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { createInterface } from "node:readline";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";

const npm = "https://registry.npmjs.org/";
const openCodeRelease = "https://github.com/anomalyco/opencode/releases/download/v2.2.7/";
const nativePackages = {
  "darwin-arm64": {
    claude: ["@anthropic-ai/claude-code-darwin-arm64/-/claude-code-darwin-arm64-2.1.322.tgz",
      "OAx5Y7V7tGCi0tWuDV3lwcDWmIua512NgpClLbqvwC9m+mbLiaytgVLXf4unTXBjMf+QbQiIUO1Kus90PklDhA=="],
    codex: ["@openai/codex/-/codex-0.167.2-darwin-arm64.tgz",
      "JB6L+L2jefDIaNgoHi7ePZKK1u2riDhlbwgm6zK57ZWpMGAYAwKgDYDb7l+xie7oUkRZuCVx307wsB/k4VsMKw=="],
    opencode: ["opencode-darwin-arm64.zip",
      "811c42be9cad489ea8c411ead49e06f691deee4957ba9a0a56a159a7d6c1abdc"],
  },
  "linux-x64": {
    claude: ["@anthropic-ai/claude-code-linux-x64/-/claude-code-linux-x64-2.1.322.tgz",
      "0DoaNAXLWCYYIGachWBCkTUsS8dOOgvk4R2fFKdrLejO9XlRTwaWUMD9ONdDmv0csdpwxj4qLUN6zWMaG2Sz6g=="],
    codex: ["@openai/codex/-/codex-0.167.2-linux-x64.tgz",
      "2Rw3VkWonMxwCSgOw+NkiKLOuOzyoQNWmxU7x8GniesdMfhKc4asNqOzTgQwJLKCrSlPc4/6fYTaH7fYcuGWGQ=="],
    opencode: ["opencode-linux-x64.tar.gz",
      "89a6cee227b54bc011179c528a5e63176032462b1a5e21723b616801cddc2ca7"],
  },
};
const platform = process.platform + "-" + process.arch;
const packages = nativePackages[platform];
assert.ok(packages, "Expected a native Apple silicon macOS or x64 Ubuntu runner");
assert.ok(process.env.RUNNER_TEMP);
const temp = await realpath(process.env.RUNNER_TEMP);
const scratch = await realpath(await mkdtemp(join(temp, "mcp-client-proof-")));
assert.equal(dirname(scratch), temp);
const exec = promisify(execFile);
const catalog = "catalog_ci";
const repository = fileURLToPath(new URL("../..", import.meta.url));
const target = resolve(process.env.CARGO_TARGET_DIR);
const builder = join(target, "debug", "mcp-builder");
const project = join(scratch, "client project");
const input = join(scratch, "builder project");
const standalone = join(scratch, "standalone export café");
const claudeConfig = join(scratch, "claude config");
const codexConfig = join(scratch, "codex config");
const tempConfig = join(scratch, "config");
const tempData = join(scratch, "data");
const tempState = join(scratch, "state");
const tempCache = join(scratch, "cache");
const childTemp = join(scratch, "temp");
const noExternalNetwork = "http://127.0.0.1:1";
const clientEnv = {
  PATH: process.env.PATH,
  LANG: process.platform === "darwin" ? "en_US.UTF-8" : "C.UTF-8",
  TMPDIR: childTemp,
  XDG_CONFIG_HOME: tempConfig,
  XDG_DATA_HOME: tempData,
  XDG_STATE_HOME: tempState,
  XDG_CACHE_HOME: tempCache,
  GIT_CONFIG_NOSYSTEM: "1",
  GIT_CONFIG_GLOBAL: join(scratch, "gitconfig"),
  NO_COLOR: "1",
  CI: "true",
  HTTP_PROXY: noExternalNetwork,
  HTTPS_PROXY: noExternalNetwork,
  ALL_PROXY: noExternalNetwork,
  NO_PROXY: "localhost,127.0.0.1,::1",
};

async function openPackage(name) {
  const [path, expected] = packages[name];
  const fromGitHub = name === "opencode";
  const archive = join(scratch, name + (path.endsWith(".zip") ? ".zip" : ".tar.gz"));
  const output = join(scratch, name);
  await mkdir(output);
  await exec("curl", ["--fail", "--silent", "--show-error", "--location", "--proto", "=https",
    "--proto-redir", "=https", "--retry", "2", "--output", archive,
    (fromGitHub ? openCodeRelease : npm) + path], { timeout: 120_000 });
  const checksum = createHash(fromGitHub ? "sha256" : "sha512");
  for await (const bytes of createReadStream(archive)) checksum.update(bytes);
  assert.equal(checksum.digest(fromGitHub ? "hex" : "base64"), expected, name + " archive integrity");
  if (path.endsWith(".zip")) {
    await exec("unzip", ["-q", archive, "-d", output]);
  } else {
    await exec("tar", ["-xzf", archive, "-C", output]);
  }
  const files = await readdir(output, { recursive: true, withFileTypes: true });
  const binaries = files.filter((file) => file.isFile() && file.name === name);
  assert.equal(binaries.length, 1, name + " native binary");
  return join(binaries[0].parentPath, name);
}

async function generateExports() {
  await Promise.all([project, input, claudeConfig, codexConfig, tempConfig, tempData, tempState, tempCache, childTemp]
    .map((path) => mkdir(path)));
  await copyFile(join(repository, "crates", "builder-project", "tests", "fixtures", "catalog-v1.json"),
    join(input, "mcp-project.json"));
  await exec("git", ["init", "--quiet", "--template=", project], { cwd: project, env: clientEnv });
  await exec(builder, ["generate", input, "--out", standalone], { timeout: 30_000 });
  try {
    await exec("cargo", ["build", "--locked"], { cwd: standalone, timeout: 600_000, maxBuffer: 4 << 20 });
  } catch (error) {
    console.error(error.stderr);
    throw error;
  }
  const executable = join(standalone, "catalog server café");
  await copyFile(join(target, "debug", "mcp-standalone-server"), executable);
  await chmod(executable, 0o700);
  const manifest = join(standalone, "mcp-project.json");
  const runtime = join(standalone, "runtime config café.json");
  const digest = createHash("sha256").update(await readFile(manifest)).digest("hex");
  await writeFile(runtime, JSON.stringify({
    configVersion: 1,
    kind: "stdio",
    manifestRevision: "sha256:" + digest,
    approvedPromptIds: ["review_item"],
  }), { flag: "wx" });
  const configs = {
    "claude-code": join(project, ".mcp.json"),
    codex: join(codexConfig, "config.toml"),
    opencode: join(project, "opencode.json"),
  };
  for (const [profile, path] of Object.entries(configs)) {
    await exec(builder, ["client", "export", profile, "--name", catalog, "--executable", executable,
      "--manifest", manifest, "--runtime-config", runtime, "--out", path], { cwd: project, timeout: 15_000 });
  }
  return { executable, manifest, runtime, configs };
}

function invoke(binary, env, args) {
  return exec(binary, args, { cwd: project, env, timeout: 25_000, maxBuffer: 1 << 20 });
}

async function checkClaude(binary) {
  const env = {
    ...clientEnv,
    CLAUDE_CONFIG_DIR: claudeConfig,
    CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC: "1",
    DISABLE_TELEMETRY: "1",
    DISABLE_ERROR_REPORTING: "1",
    DISABLE_AUTOUPDATER: "1",
    ENABLE_CLAUDEAI_MCP_SERVERS: "false",
  };
  const { stdout: version } = await invoke(binary, env, ["--version"]);
  assert.equal(version.trim(), "2.1.322 (Claude Code)");
  await writeFile(join(claudeConfig, "settings.json"), JSON.stringify({ enabledMcpjsonServers: [catalog] }),
    { flag: "wx" });
  const { stdout } = await invoke(binary, env, ["mcp", "list"]);
  assert.match(stdout, /catalog_ci: .+Connected/i);
  console.log("Claude Code 2.1.322 connected to the exported stdio server");
}

async function checkCodex(binary, paths) {
  const env = { ...clientEnv, CODEX_HOME: codexConfig };
  const { stdout: version } = await invoke(binary, env, ["--version"]);
  assert.equal(version.trim(), "codex-cli 0.167.2");
  const { stdout } = await invoke(binary, env, ["mcp", "get", catalog, "--json"]);
  const { name, transport } = JSON.parse(stdout);
  assert.equal(name, catalog);
  assert.equal(transport.type, "stdio");
  assert.deepEqual([transport.args[0], transport.args[2]], ["--manifest", "--config"]);
  for (const [actual, expected] of [
    [transport.command, paths.executable], [transport.args[1], paths.manifest], [transport.args[3], paths.runtime],
  ]) assert.equal(await realpath(actual), await realpath(expected));

  const child = spawn(binary, ["app-server"], {
    cwd: project, env, stdio: ["pipe", "pipe", "inherit"],
  });
  const exited = once(child, "close");
  const lines = createInterface({ input: child.stdout });
  const replies = lines[Symbol.asyncIterator]();
  const timer = setTimeout(() => child.kill("SIGKILL"), 20_000);
  child.stdin.on("error", () => lines.close());
  let nextId = 0;
  const send = (message) => child.stdin.write(JSON.stringify(message) + "\n");
  async function request(method, params) {
    const id = ++nextId;
    send({ id, method, params });
    for (;;) {
      const { value, done } = await replies.next();
      assert.ok(!done, "Codex app-server closed before " + method);
      const frame = JSON.parse(value);
      if (frame.id === id) {
        assert.ok(frame.result, JSON.stringify(frame.error));
        return frame.result;
      }
    }
  }
  try {
    await request("initialize", {
      clientInfo: { name: "native-client-proof", version: "1" },
      capabilities: { experimentalApi: true },
    });
    send({ method: "initialized" });
    const status = await request("mcpServerStatus/list", { serverName: catalog, detail: "full" });
    const server = status.data.find((entry) => entry.name === catalog);
    assert.equal(server?.serverInfo?.name, "catalog");
    assert.deepEqual(server.serverCapabilities?.prompts, {});
    console.log("Codex CLI 0.167.2 connected to the exported stdio server and read the prompt capability");
  } finally {
    clearTimeout(timer);
    child.stdin.end();
    lines.close();
    if (child.exitCode === null && child.signalCode === null) child.kill();
    await exited;
  }
}

async function checkOpenCode(binary) {
  const env = {
    ...clientEnv,
    OPENCODE_DISABLE_MODELS_FETCH: "true",
    OPENCODE_DISABLE_AUTOUPDATE: "true",
    OPENCODE_DISABLE_DEFAULT_PLUGINS: "true",
    OPENCODE_DISABLE_TERMINAL_TITLE: "true",
  };
  const { stdout: version } = await invoke(binary, env, ["--version"]);
  assert.equal(version.trim(), "opencode v2.2.7");
  let result;
  try {
    const deadline = Date.now() + 20_000;
    do {
      result = (await invoke(binary, env, ["mcp", "list"])).stdout;
      if (/catalog_ci\s+connected/i.test(result)) {
        console.log("OpenCode V2 2.2.7 connected to the exported stdio server");
        return;
      }
      await delay(500);
    } while (Date.now() < deadline);
    assert.fail("OpenCode did not report the configured server connected: " + result);
  } finally {
    await invoke(binary, env, ["service", "stop"]);
  }
}

try {
  console.log("Native runner: " + platform + " using Node " + process.version);
  const paths = await generateExports();
  const before = await Promise.all(Object.values(paths.configs).map((path) => readFile(path)));
  const checks = await Promise.allSettled([
    openPackage("claude").then(checkClaude),
    openPackage("codex").then((binary) => checkCodex(binary, paths)),
    openPackage("opencode").then(checkOpenCode),
  ]);
  const errors = checks.filter((check) => check.status === "rejected").map((check) => check.reason);
  if (errors.length > 0) throw new AggregateError(errors, "Native client check failed on " + platform);
  const after = await Promise.all(Object.values(paths.configs).map((path) => readFile(path)));
  assert.deepEqual(after, before);
  console.log("All three official clients connected using unmodified v0.3.0 configuration exports");
} finally {
  await rm(scratch, { recursive: true, force: true });
}
