import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  mkdtempSync,
  readFileSync,
  realpathSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join } from "node:path";

assert.equal(
  process.argv.length,
  4,
  "Supply the generated standalone executable and catalog manifest",
);
const [server, manifest] = process.argv
  .slice(2)
  .map((path) => realpathSync(path));
assert.match(basename(server), /^mcp-standalone-server(?:\.exe)?$/u);

const manifestBytes = readFileSync(manifest);
const confirm = JSON.parse(manifestBytes);
assert.ok(confirm.policy.grants.some(({ id }) => id === "catalog_read"));
assert.ok(confirm.policy.grants.some(({ id }) => id === "note_read"));
assert.ok(confirm.secretRefs.some(({ id }) => id === "catalog_auth"));
const manifestRevision =
  "sha256:" + createHash("sha256").update(manifestBytes).digest("hex");

const parent = realpathSync(tmpdir());
const scratch = realpathSync(
  mkdtempSync(join(parent, "mcp-standalone-admission-")),
);
assert.equal(dirname(scratch), parent);

function launch(args, expectedCode) {
  const result = spawnSync(server, args, {
    cwd: scratch,
    encoding: "utf8",
    input: "",
    timeout: 10_000,
    windowsHide: true,
  });
  assert.ifError(result.error);
  assert.equal(result.signal, null);
  assert.equal(result.status, expectedCode ? 1 : 0);
  assert.equal(
    result.stdout,
    "",
    "Operator configuration must not enter the protocol stream",
  );
  assert.equal(
    result.stderr.trim(),
    expectedCode ?? "",
    "Report only the stable diagnostic code",
  );
}

try {
  const invalidManifest = join(scratch, "invalid project.json");
  writeFileSync(invalidManifest, "{}");
  launch(["--manifest", invalidManifest], "project.invalid_manifest");
  launch(
    ["--manifest"],
    "standalone.usage\nUsage: mcp-standalone-server [--manifest PATH] [--config PATH]",
  );
  launch(
    ["--manifest", join(scratch, "missing project.json")],
    "project.manifest_unavailable",
  );
  const args = ["--manifest", manifest, "--config"];
  launch(
    [...args, join(scratch, "missing config.json")],
    "runtime_config.unavailable",
  );

  const file = join(scratch, "operator config.json");
  const config = { configVersion: 1, kind: "stdio", manifestRevision };
  const cases = [
    [{ approvedGrantIds: ["unknown_grant"] }, "runtime_config.unknown_grant"],
    [
      { approvedGrantIds: ["catalog_read", "catalog_read"] },
      "runtime_config.duplicate_grant",
    ],
    [{ approvedPromptIds: ["lookup_item"] }, "runtime_config.unknown_prompt"],
    [
      { approvedPromptIds: ["review_item", "review_item"] },
      "runtime_config.duplicate_prompt",
    ],
    [
      { approvedGrantIds: ["note_read"] },
      "runtime_config.target_binding_required",
    ],
    [
      { targetBindings: { other: { kind: "fileRead", path: scratch } } },
      "runtime_config.unknown_target",
    ],
    [
      {
        targetBindings: {
          catalog_docs: { kind: "fileRead", path: "relative" },
        },
      },
      "runtime_config.invalid_target_path",
    ],
    [
      {
        approvedGrantIds: ["note_read"],
        targetBindings: {
          catalog_docs: { kind: "fileRead", path: join(scratch, "missing") },
        },
      },
      "adapter.file_root_unavailable",
    ],
    [
      {
        secretSources: {
          unknown_secret: { kind: "env", name: "MCP_TEST_TOKEN" },
        },
      },
      "runtime_config.unknown_secret",
    ],
    [
      {
        secretSources: { catalog_auth: { kind: "env", name: "invalid name" } },
      },
      "runtime_config.invalid_environment_name",
    ],
    [
      {
        secretSources: {
          catalog_auth: { kind: "env", name: "MCP_TEST_TOKEN" },
        },
      },
      undefined,
    ],
  ];
  for (const [values, code] of cases) {
    writeFileSync(file, JSON.stringify({ ...config, ...values }));
    launch([...args, file], code);
  }
  console.log("Standalone manifest and operator configuration checks passed.");
} finally {
  rmSync(scratch, { recursive: true, force: true });
}
