import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { copyFileSync, mkdirSync, mkdtempSync, realpathSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repository = fileURLToPath(new URL("../..", import.meta.url));
const target = resolve(process.env.CARGO_TARGET_DIR ?? join(repository, "target"));
const executable = process.platform === "win32" ? "mcp-builder.exe" : "mcp-builder";
const builder = join(target, "debug", executable);
const parent = realpathSync(tmpdir());
const scratch = realpathSync(mkdtempSync(join(parent, "mcp-standalone-ci-")));
assert.equal(dirname(scratch), parent);

try {
  const project = join(scratch, "project");
  const output = join(scratch, "standalone export");
  mkdirSync(project);
  copyFileSync(
    join(repository, "crates", "builder-project", "tests", "fixtures", "catalog-v1.json"),
    join(project, "mcp-project.json"),
  );
  const result = JSON.parse(execFileSync(builder, ["generate", project, "--out", output, "--json"], {
    encoding: "utf8", windowsHide: true,
  }));
  assert.equal(result.version, 1);
  assert.equal(result.command, "generate");
  assert.equal(result.ok, true);
  assert.equal(result.data.published, true);
  assert.ok(result.data.generation.files.some(file => file.path === "tests/stdio.rs"));

  const env = { ...process.env, CARGO_TARGET_DIR: target };
  for (const command of ["build", "test"]) {
    const args = [command, "--locked"];
    if (command === "test") args.push("--workspace");
    execFileSync("cargo", args, { cwd: output, env, stdio: "inherit", windowsHide: true });
  }
} finally {
  rmSync(scratch, { recursive: true, force: true });
}
