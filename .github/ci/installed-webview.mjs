import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { createServer } from "node:net";
import { dirname, resolve } from "node:path";
import { setTimeout as delay } from "node:timers/promises";

assert.equal(process.platform, "win32");
assert.equal(process.env.GITHUB_ACTIONS, "true");
assert.equal(process.env.RUNNER_ENVIRONMENT, "github-hosted");
const [driverInput, binaryInput, profileInput] = process.argv.slice(2);
assert.ok(driverInput && binaryInput && profileInput, "Supply the driver, installed executable, and profile.");
const driver = resolve(driverInput);
const binary = resolve(binaryInput);
const profile = resolve(profileInput);
const marker = "m10-installed-webview-synthetic-marker";
const listener = createServer().listen(0, "127.0.0.1");
await once(listener, "listening");
const port = listener.address().port;
await new Promise((done) => listener.close(done));

const server = spawn(driver, [`--port=${port}`, "--allowed-ips=127.0.0.1"], {
  cwd: dirname(binary),
  env: { ...process.env, WEBVIEW2_USER_DATA_FOLDER: profile },
  stdio: ["ignore", "pipe", "pipe"],
  windowsHide: true,
});
let log = "";
let spawnError;
server.on("error", (error) => { spawnError = error; });
for (const stream of [server.stdout, server.stderr]) {
  stream.setEncoding("utf8").on("data", (text) => { log = (log + text).slice(-8192); });
}
const closed = new Promise((done) => server.once("close", done));

async function until(label, predicate) {
  const deadline = Date.now() + 45_000;
  do {
    if (spawnError) throw spawnError;
    assert.equal(server.exitCode, null, "Edge WebDriver exited during the installed workflow");
    const result = await predicate();
    if (result) return result;
    await delay(150);
  } while (Date.now() < deadline);
  throw new Error(`Timed out waiting for ${label}.`);
}

let session;
async function request(method, path, data, timeout = 10_000) {
  const response = await fetch(`http://127.0.0.1:${port}${path}`, {
    method,
    headers: data === undefined ? undefined : { "content-type": "application/json" },
    body: data === undefined ? undefined : JSON.stringify(data),
    signal: AbortSignal.timeout(timeout),
  });
  const { value } = await response.json();
  if (!response.ok || value?.error) {
    throw new Error(`Edge WebDriver ${method} ${path}: ${value?.message ?? response.status}`);
  }
  return value;
}

const evaluate = (fn, ...args) => request("POST", `${session}/execute/sync`, {
  script: `return (${fn.toString()})(...arguments);`, args,
});

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

try {
  await until("Edge WebDriver status", async () => {
    try {
      return (await request("GET", "/status", undefined, 1200)).ready;
    } catch (error) {
      if (error.cause?.code === "ECONNREFUSED") return false;
      throw error;
    }
  });
  const { sessionId } = await request("POST", "/session", {
    capabilities: { alwaysMatch: { browserName: "webview2", "ms:edgeOptions": { binary } } },
  }, 60_000);
  assert.match(sessionId, /^[\da-f-]{16,64}$/i);
  session = `/session/${sessionId}`;
  await until("the installed local page", async () => {
    const url = URL.parse(await request("GET", `${session}/url`));
    return url?.hostname === "tauri.localhost" && ["http:", "https:"].includes(url.protocol);
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
  assert.match(await textAt("app-playground"), /Sample item/);
  await until("protocol trace details", () => evaluate(() => {
    const summary = [...document.querySelectorAll("app-playground summary")].find(
      (item) => item.textContent.trim() === "Protocol trace",
    );
    if (!summary) return false;
    if (!summary.parentElement.open) summary.click();
    return true;
  }));
  await click("Refresh trace");
  await until("tool traffic in the protocol trace", async () =>
    (await textAt(".trace-list")).includes("tools/call"));
  assert.ok(!(await textAt("app-playground")).includes(marker), "Synthetic private field reached the UI");
  await click("Stop server");
  await status("Local server stopped.");
  console.log("Installed WebView2 opened the catalog, cancelled a review, replayed a fixture and stopped its bundled worker.");
} catch (error) {
  if (log) console.error(log);
  throw error;
} finally {
  if (session) await request("DELETE", session, undefined, 6000).catch(() => {});
  if (server.exitCode === null && server.signalCode === null) server.kill();
  await Promise.race([closed, delay(5000, undefined, { ref: false })]);
}
