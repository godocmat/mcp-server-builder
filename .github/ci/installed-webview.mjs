import assert from "node:assert/strict";
import { execFile, spawn } from "node:child_process";
import { once } from "node:events";
import { basename, dirname, resolve } from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { promisify } from "node:util";

assert.equal(process.platform, "win32");
assert.equal(process.env.GITHUB_ACTIONS, "true");
assert.equal(process.env.RUNNER_ENVIRONMENT, "github-hosted");
const [binaryInput, portInput] = process.argv.slice(2);
assert.ok(binaryInput, "Supply the installed executable.");
const binary = resolve(binaryInput);
assert.equal(basename(binary).toLowerCase(), "mcp-builder-desktop.exe");
const port = Number(portInput);
assert.ok(Number.isInteger(port) && port >= 1024 && port <= 65535, "Supply the loopback debugging port.");
const marker = "m10-installed-webview-synthetic-marker";
const run = promisify(execFile);

const app = spawn(binary, [], {
  cwd: dirname(binary),
  stdio: ["ignore", "ignore", "pipe"],
});
let stderr = "";
let spawnError;
app.on("error", (error) => { spawnError = error; });
app.stderr.setEncoding("utf8").on("data", (text) => { stderr = (stderr + text).slice(-8192); });
const closed = new Promise((done) => app.once("close", done));

async function inspectWindow() {
  if (!app.pid || app.exitCode !== null) return;
  try {
    const { stdout } = await run("pwsh.exe", [
      "-NoProfile", "-NonInteractive", "-Command",
      `Get-Process -Id ${app.pid} -ErrorAction Stop | Select-Object Id,SessionId,Responding,@{Name='MainWindowHandle';Expression={$_.MainWindowHandle.ToInt64()}} | ConvertTo-Json -Compress`,
    ], { timeout: 5000, windowsHide: true });
    console.error(`Installed desktop process: ${stdout.trim()}`);
  } catch (error) {
    console.error(`Installed desktop PID ${app.pid}; window diagnostics unavailable (${error.code ?? error.name}).`);
  }
}

async function until(label, predicate) {
  const deadline = Date.now() + 45_000;
  do {
    if (spawnError) throw spawnError;
    assert.equal(app.exitCode, null, "Installed desktop exited during its workflow");
    assert.equal(app.signalCode, null, "Installed desktop was terminated during its workflow");
    const result = await predicate();
    if (result) return result;
    await delay(150);
  } while (Date.now() < deadline);
  throw new Error(`Timed out waiting for ${label}.`);
}

let socket;
let nextId = 0;
const pending = new Map();

async function evaluate(fn, ...args) {
  const id = ++nextId;
  const response = await new Promise((resolveResponse, reject) => {
    const timer = setTimeout(() => {
      pending.delete(id);
      reject(new Error("The installed WebView did not answer its test client."));
    }, 6_000);
    pending.set(id, (message) => {
      clearTimeout(timer);
      resolveResponse(message);
    });
    const expression = `(${fn.toString()})(${args.map((arg) => JSON.stringify(arg)).join(",")})`;
    socket.send(JSON.stringify({
      id, method: "Runtime.evaluate", params: { expression, returnByValue: true },
    }));
  });
  if (response.error || response.result?.exceptionDetails) {
    throw new Error("An installed WebView test action raised an exception.");
  }
  return response.result.result.value;
}

const click = (label) => until(label, () => evaluate((expected) => {
  const normalized = (button) => button.textContent.replace(/\s+/g, " ").trim();
  const button = [...document.querySelectorAll("button")].find(
    (item) => !item.disabled && item.getClientRects().length && normalized(item) === expected,
  );
  if (!button) return false;
  button.click();
  return true;
}, label));

const textAt = (selector) => evaluate((query) => document.querySelector(query)?.innerText ?? "", selector);
const status = (expected) => until(expected, async () =>
  (await textAt("app-playground p.status")).includes(expected));

let endpointProbe = "no response";
try {
  console.log(`Launching the installed desktop (PID ${app.pid ?? "unavailable"}) with WebView2 on loopback port ${port}.`);
  const page = await until("the installed WebView2 page", async () => {
    try {
      const response = await fetch(`http://127.0.0.1:${port}/json/list`, {
        redirect: "error",
        signal: AbortSignal.timeout(1500),
      });
      if (!response.ok) {
        endpointProbe = `HTTP ${response.status}`;
        return null;
      }
      const targets = await response.json();
      endpointProbe = targets.map(({ type }) => type).join(", ") || "empty target list";
      return targets.find((item) => {
        const url = URL.parse(item.url);
        return ["page", "webview"].includes(item.type) && item.webSocketDebuggerUrl &&
          url?.hostname === "tauri.localhost" && ["http:", "https:"].includes(url.protocol);
      });
    } catch (error) {
      endpointProbe = error.cause?.code ?? error.name;
      return null;
    }
  });
  const endpoint = new URL(page.webSocketDebuggerUrl);
  assert.equal(endpoint.protocol, "ws:");
  assert.ok(["127.0.0.1", "localhost"].includes(endpoint.hostname));
  endpoint.hostname = "127.0.0.1";
  assert.equal(endpoint.port, String(port));
  socket = new WebSocket(endpoint);
  await once(socket, "open", { signal: AbortSignal.timeout(8000) });
  console.log("Connected to the installed WebView2 page.");
  socket.addEventListener("message", ({ data }) => {
    const response = JSON.parse(data);
    const done = pending.get(response.id);
    if (!done) return;
    pending.delete(response.id);
    done(response);
  });

  await until("the registered catalog", () => evaluate(() => {
    const catalog = [...document.querySelectorAll(".project-card")].find(
      (card) => !card.disabled && card.querySelector("strong")?.textContent.trim() === "Catalog sample",
    );
    if (!catalog) return false;
    catalog.click();
    return true;
  }));
  await click("Playground");
  const selectPermissions = () => until("fixture and trace controls", () => evaluate(() => {
    const fixture = document.querySelector("#fixture-scenario");
    const inputs = [...document.querySelectorAll("app-playground label")];
    const grant = inputs.find((label) => label.textContent.includes("catalog_read"))?.querySelector("input");
    const trace = inputs.find((label) => label.textContent.includes("Include redacted"))?.querySelector("input");
    if (!fixture || !grant || !trace) return false;
    fixture.value = "known_item";
    fixture.dispatchEvent(new Event("change", { bubbles: true }));
    if (!grant.checked) grant.click();
    if (!trace.checked) trace.click();
    return true;
  }));
  await selectPermissions();
  await click("Review launch");
  await click("Cancel launch review");
  await status("Launch review cancelled.");
  await selectPermissions();
  await click("Review launch");
  await until("the reviewed bundled worker", async () =>
    (await textAt("app-playground")).includes("mcp-builder.exe"));
  await click("Approve and start");
  await status("Local server connected.");
  await click("Run saved fixture");
  await until("the fixture assertion pass", async () =>
    (await textAt("#preview-result-heading")).includes("Passed"));
  await until("the fixture's redacted result", () => evaluate(() =>
    document.querySelector('section[aria-labelledby="preview-result-heading"] pre')
      ?.textContent.includes("Sample item")));
  await until("protocol trace details", () => evaluate(() => {
    const summary = [...document.querySelectorAll("app-playground summary")].find(
      (item) => item.textContent.trim() === "Protocol trace",
    );
    if (!summary) return false;
    if (!summary.parentElement.open) summary.click();
    return true;
  }));
  await click("Refresh trace");
  await until("the captured tool result in the protocol trace", () => evaluate(() =>
    [...document.querySelectorAll(".trace-list li > details")].some((entry) =>
      entry.querySelector("summary")?.textContent.includes("tools/call") &&
      entry.querySelector("pre")?.textContent.includes("Sample item"))));
  const markerExposed = await evaluate((value) =>
    document.querySelector("app-playground")?.textContent.includes(value), marker);
  assert.equal(markerExposed, false, "Synthetic private field reached the UI");
  await click("Stop server");
  await status("Local server stopped.");
  assert.ok(!stderr.includes(marker), "Synthetic private field reached process stderr");
  console.log("Installed WebView2 opened the catalog, cancelled a review, replayed a fixture and stopped its bundled worker.");
} catch (error) {
  console.error(`Last WebView2 endpoint observation: ${endpointProbe}`);
  await inspectWindow();
  if (stderr) console.error(stderr);
  throw error;
} finally {
  socket?.close();
  if (app.exitCode === null && app.signalCode === null) app.kill();
  const ended = await Promise.race([closed.then(() => true), delay(5000, false, { ref: false })]);
  if (!ended) {
    app.stderr.destroy();
    app.unref();
    throw new Error("The installed desktop did not exit during cleanup.");
  }
}
