import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { mkdtemp, readFile, realpath, rm } from "node:fs/promises";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { setTimeout as delay } from "node:timers/promises";

const fixture = JSON.parse(
  await readFile(new URL("../../crates/builder-app/tests/fixtures/workspace-smoke.json", import.meta.url), "utf8"),
);
const { builderVersion, contractVersion } = fixture.supported.reply.data;
const target = await realpath(resolve(process.env.CARGO_TARGET_DIR ?? "target"));
const binary = join(target, "debug", process.platform === "win32" ? "mcp-builder-desktop.exe" : "mcp-builder-desktop");
const socket = createServer().listen(0, "127.0.0.1");
await once(socket, "listening");
const port = socket.address().port;
await new Promise((done) => socket.close(done));

const profileParent = await realpath(tmpdir());
const profile = await mkdtemp(join(profileParent, "mcp-builder-webdriver-"));
const app = spawn(binary, [], {
  env: {
    ...process.env,
    TAURI_WEBDRIVER_PORT: String(port),
    RUST_LOG: "builder_app=info",
    WEBVIEW2_USER_DATA_FOLDER: profile,
    XDG_CACHE_HOME: join(profile, "cache"),
    XDG_CONFIG_HOME: join(profile, "config"),
    XDG_DATA_HOME: join(profile, "data"),
  },
  stdio: ["ignore", "ignore", "pipe"],
  windowsHide: true,
});
let appError;
app.on("error", (error) => { appError = error; });
const closed = new Promise((done) => app.once("close", done));
let stderr = "";
app.stderr.setEncoding("utf8").on("data", (data) => { stderr = (stderr + data).slice(-32_768); });

async function request(method, path, data) {
  const response = await fetch(`http://127.0.0.1:${port}${path}`, {
    method,
    headers: data === undefined ? undefined : { "Content-Type": "application/json" },
    body: JSON.stringify(data),
    signal: AbortSignal.timeout(15_000),
  });
  const { value } = await response.json();
  if (!response.ok || value?.error) {
    throw new Error(`${method} ${path}: ${response.status} ${JSON.stringify(value)}`);
  }
  return value;
}

async function waitFor(description, check) {
  const deadline = Date.now() + 25_000;
  do {
    if (appError) throw appError;
    assert.equal(app.exitCode, null, "Tauri exited before the smoke completed");
    assert.equal(app.signalCode, null, "Tauri was terminated before the smoke completed");
    const result = await check();
    if (result) return result;
    await delay(250);
  } while (Date.now() < deadline);
  throw new Error(`Timed out waiting for ${description}`);
}

let session;
try {
  await waitFor("the native WebView window", async () => {
    try {
      return (await request("GET", "/status")).ready;
    } catch (error) {
      if (error.cause?.code !== "ECONNREFUSED") throw error;
      return false;
    }
  });
  const { sessionId, capabilities } = await request("POST", "/session", {
    capabilities: { alwaysMatch: { "wdio:tauriServiceOptions": { windowLabel: "main" } } },
  });
  assert.ok(sessionId, "WebDriver must open a session");
  session = `/session/${sessionId}`;
  const elementKey = "element-6066-11e4-a52e-4f735466cecf";
  const find = async (selector) => {
    const elements = await request("POST", `${session}/elements`, { using: "css selector", value: selector });
    return elements[0]?.[elementKey];
  };
  const button = await waitFor("the Angular smoke button", () => find("app-workspace-smoke button"));
  const url = await request("GET", `${session}/url`);
  assert.match(url, /^(?:tauri:\/\/localhost|https?:\/\/tauri\.localhost)\/?$/);
  assert.equal((await request("GET", `${session}/element/${button}/text`)).trim(), "Run workspace check");
  await request("POST", `${session}/element/${button}/click`, {});

  const status = await waitFor("the rendered IPC reply", async () => {
    const element = await find('app-workspace-smoke [role="status"]');
    if (!element) return false;
    const message = await request("GET", `${session}/element/${element}/text`);
    if (message.includes("Could not reach the desktop service.")) throw new Error(message);
    return message.includes("Workspace check passed.") && message;
  });
  const fields = await request("POST", `${session}/execute/sync`, {
    script: `
      return {
        userAgent: navigator.userAgent,
        reply: Object.fromEntries(Array.from(document.querySelectorAll('[role="status"] dt'), (label) => [
          label.textContent,
          label.nextElementSibling.textContent,
        ])),
      };
    `,
    args: [],
  });
  assert.match(fields.userAgent, process.platform === "win32" ? /\bEdg\/\d+/ : /\bAppleWebKit\/\d+/);
  assert.equal(fields.reply.Builder, builderVersion, status);
  assert.equal(fields.reply["IPC contract"], String(contractVersion), status);
  const correlationId = fields.reply["Correlation ID"];
  assert.match(correlationId, /^[\da-f]{8}-[\da-f]{4}-4[\da-f]{3}-[89ab][\da-f]{3}-[\da-f]{12}$/i);
  await waitFor("the matching Rust trace", () => stderr.split("\n").some((line) => {
    if (!line.includes("Workspace smoke completed")) return false;
    const trace = JSON.parse(line);
    return trace.target === "builder_app::smoke" && trace.fields.correlation_id === correlationId;
  }));
  console.log(`Native smoke passed at ${url} (${capabilities.browserName} ${capabilities.browserVersion}, contract ${contractVersion}, correlation ${correlationId}).`);
  console.log(`WebView user agent: ${fields.userAgent}`);
} catch (error) {
  if (stderr) console.error(stderr.trim());
  throw error;
} finally {
  try {
    if (session && app.exitCode === null && app.signalCode === null) await request("DELETE", session);
  } catch (error) {
    console.warn(`WebDriver session cleanup: ${error.message}`);
  } finally {
    app.stderr.destroy();
    app.kill();
    if (await Promise.race([closed.then(() => false), delay(5_000, true, { ref: false })])) {
      app.kill("SIGKILL");
      await closed;
    }
    const resolvedProfile = await realpath(profile);
    assert.equal(dirname(resolvedProfile), profileParent);
    await rm(resolvedProfile, { recursive: true, force: true, maxRetries: 8, retryDelay: 250 });
  }
}
