import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";

const marker = "native-gui-fixture-private-marker";
const elementKey = "element-6066-11e4-a52e-4f735466cecf";

export async function registerCatalog(binary, profile, env) {
  const project = join(profile, "catalog-fixture");
  const fixture = JSON.parse(await readFile(
    new URL("../../crates/builder-project/tests/fixtures/catalog-v1.json", import.meta.url), "utf8",
  ));
  const scenario = fixture.scenarios.find((entry) => entry.id === "known_item");
  scenario.fixtures[0].body.private = marker;
  await mkdir(project);
  await writeFile(join(project, "mcp-project.json"), JSON.stringify(fixture));
  const worker = join(dirname(binary), process.platform === "win32" ? "mcp-builder.exe" : "mcp-builder");
  const reply = JSON.parse(execFileSync(worker, ["--json", "project", "open", project], {
    env, encoding: "utf8", timeout: 30_000,
  }));
  assert.equal(reply.ok, true, "The sibling CLI should register the catalog fixture.");
  assert.equal(reply.data.document.name, "Catalog sample");
}

export async function exercisePlayground(request, session, waitFor) {
  const execute = (script, ...args) => request("POST", `${session}/execute/sync`, { script, args });
  const find = async (using, value) => {
    const elements = await request("POST", `${session}/elements`, { using, value });
    return elements[0]?.[elementKey];
  };
  const readyElement = (using, value, description) => waitFor(description, async () => {
      const element = await find(using, value);
      if (!element || !await request("GET", `${session}/element/${element}/displayed`)) return false;
      return await request("GET", `${session}/element/${element}/enabled`) && element;
  });
  const click = async (using, value, description) => {
    const id = await readyElement(using, value, description);
    await request("POST", `${session}/element/${id}/click`, {});
  };
  const check = async (value, description) => {
    const id = await readyElement("xpath", value, description);
    if (!await request("GET", `${session}/element/${id}/selected`)) {
      await request("POST", `${session}/element/${id}/click`, {});
    }
    assert.equal(await request("GET", `${session}/element/${id}/selected`), true, description);
  };
  const choose = async (selector, value) => {
    await waitFor(`${selector} availability`, () => execute(
      "const select = document.querySelector(arguments[0]); return !!select?.getClientRects().length && !select.matches(':disabled');",
      selector,
    ));
    assert.equal(await execute(`
      const select = document.querySelector(arguments[0]);
      select.value = arguments[1];
      select.dispatchEvent(new Event('change', { bubbles: true }));
      return select.value;
    `, selector, value), value);
  };
  const contains = (selector, expected, description) => waitFor(description, () => execute(
    "const element = document.querySelector(arguments[0]); return !!element?.getClientRects().length && element.innerText.includes(arguments[1]);",
    selector, expected,
  ));
  const playground = "app-playground";
  const review = `${playground} section[aria-labelledby=preview-review-heading]`;
  const result = `${playground} section[aria-labelledby=preview-result-heading]`;
  const trace = `${playground} .trace-list`;

  try {
    await click("xpath", "//button[contains(@class, 'project-card') and strong[normalize-space(.)='Catalog sample']]", "the catalog project card");
    await click("xpath", "//nav[@aria-label='Workspace']//button[normalize-space(.)='Playground']", "the Playground tab");
    for (const revision of ["2026-07-28", "2025-11-25"]) {
      await choose("#fixture-scenario", "known_item");
      await choose("#preview-protocol", revision);
      await check("//app-playground//label[span[starts-with(normalize-space(.), 'catalog_read')]]/input", "the catalog_read grant");
      await check("//app-playground//label[span[normalize-space(.)='Include redacted message contents in the protocol trace']]/input", "trace payload capture");
      await click("css selector", `${playground} form button[type=submit]`, "Review launch");
      await contains(review, "known_item", "the selected fixture in the launch review");
      await contains(review, "catalog_read", "the selected permission in the launch review");
      await contains(review, "/mcp-builder", "the sibling CLI executable in the launch review");
      await click("xpath", "//app-playground//button[normalize-space(.)='Approve and start']", "Approve and start");
      await contains(`${playground} .connection-info`, revision, `an MCP ${revision} connection`);
      await contains(`${playground} .discovery`, "lookup_item", "the approved discovered tool");
      await click("xpath", "//app-playground//button[normalize-space(.)='Run saved fixture']", "Run saved fixture");
      await contains("#preview-result-heading", "Passed", "fixture completion");
      await contains(result, "Assertion 1", "the fixture assertion");
      await contains(`${result} pre`, "Sample item", "the redacted displayed fixture result");
      await click("xpath", "//app-playground//details/summary[normalize-space(.)='Protocol trace']", "Protocol trace");
      await click("xpath", "//app-playground//button[normalize-space(.)='Refresh trace']", "Refresh trace");
      await contains(trace, "tools/call", "the rendered protocol trace");
      const evidence = await execute(
        "return [document.querySelector(arguments[0]).textContent, document.querySelector(arguments[1]).textContent];",
        result, trace,
      );
      assert.ok(evidence[1].includes("Sample item"), "Trace should include the sanitized response payload.");
      assert.ok(evidence.every((value) => !value.includes(marker)), "Result and trace should redact the synthetic marker.");
      await click("xpath", "//app-playground//button[normalize-space(.)='Stop server']", "Stop server");
      await contains(`${playground} .status`, "Local server stopped.", `clean shutdown after MCP ${revision}`);
      console.log(`Native GUI playground passed for MCP ${revision}: reviewed worker and grant, replayed fixture, inspected redacted trace, stopped worker.`);
    }
  } catch (error) {
    try {
      console.error(await execute("return document.querySelector('main')?.innerText.slice(0, 8000);"));
    } catch (diagnosticError) {
      console.warn(`Reading GUI failure state: ${diagnosticError.message}`);
    }
    throw error;
  }
}
