import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { copyFileSync, mkdirSync, mkdtempSync, realpathSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { checkClientConfigurations } from "./client-configuration-smoke.mjs";

const repository = fileURLToPath(new URL("../..", import.meta.url));
const target = resolve(process.env.CARGO_TARGET_DIR ?? join(repository, "target"));
const executable = process.platform === "win32" ? "mcp-builder.exe" : "mcp-builder";
const builder = process.argv[2] ? resolve(process.argv[2]) : join(target, "debug", executable);
const parent = realpathSync(tmpdir());
const scratch = realpathSync(mkdtempSync(join(parent, "mcp-standalone-ci-")));
assert.equal(dirname(scratch), parent);

function builderJson(command, args) {
  const result = JSON.parse(execFileSync(builder, [...args, "--json"], {
    encoding: "utf8", windowsHide: true, timeout: 60_000,
  }));
  assert.equal(result.version, 1);
  assert.equal(result.command, command);
  assert.equal(result.ok, true);
  return result.data;
}

try {
  const project = join(scratch, "project");
  const output = join(scratch, "standalone export");
  mkdirSync(project);
  copyFileSync(
    join(repository, "crates", "builder-project", "tests", "fixtures", "catalog-v1.json"),
    join(project, "mcp-project.json"),
  );
  assert.equal(builderJson("project.validate", ["project", "validate", project]).valid, true);
  const inputs = [project, "--all", "--grant", "catalog_read", "--grant", "note_read",
    "--prompt", "review_item"];
  const { digest } = builderJson("policy.review.launch", ["policy", "review", "launch", ...inputs]);
  assert.match(digest, /^sha256:[a-f\d]{64}$/);
  for (let replay = 0; replay < 2; replay++) {
    const { runs } = builderJson("test.run", ["test", "run", ...inputs, "--approve", digest]);
    assert.deepEqual(runs.map(run => run.scenarioId), ["known_item", "invalid_item", "note", "review"]);
    assert.ok(runs.every(run => run.outcome === "passed"));
  }
  console.log("The four catalog scenarios passed two independent managed replays.");

  const result = builderJson("generate", ["generate", project, "--out", output]);
  assert.equal(result.published, true);
  assert.ok(result.generation.files.some(file => file.path === "tests/stdio.rs"));

  const env = { ...process.env, CARGO_TARGET_DIR: target };
  for (const command of ["build", "test"]) {
    const args = [command, "--locked"];
    if (command === "test") args.push("--workspace");
    execFileSync("cargo", args, { cwd: output, env, stdio: "inherit", windowsHide: true });
  }
  const standalone = join(target, "debug", process.platform === "win32"
    ? "mcp-standalone-server.exe" : "mcp-standalone-server");
  await checkClientConfigurations({ builder, standalone, output, scratch });
  if (process.env.MCP_BUILDER_CI_INSPECTOR_LAUNCHER) {
    execFileSync(process.execPath, [
      fileURLToPath(new URL("./standalone-inspector-smoke.mjs", import.meta.url)),
      process.env.MCP_BUILDER_CI_INSPECTOR_LAUNCHER,
      standalone,
      builder,
    ], { stdio: "inherit", windowsHide: true, timeout: 120_000 });
  }
} finally {
  rmSync(scratch, { recursive: true, force: true });
}
