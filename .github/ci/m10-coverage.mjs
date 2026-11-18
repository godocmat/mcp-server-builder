import assert from "node:assert/strict";
import { globSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";

const root = fileURLToPath(new URL("../../", import.meta.url));
const { values: reports } = parseArgs({
  options: Object.fromEntries(
    ["rust", "angular", "standalone"].map((name) => [
      name,
      { type: "string", multiple: true, default: [] },
    ]),
  ),
});
assert.ok(
  reports.rust.length && reports.angular.length,
  "Supply --rust and --angular LCOV paths. Add --standalone for the generated shell.",
);

// The inventory lists whole authored modules, including their helper and error paths.
// Emitted copies under standalone crates/ use the workspace rows, never the standalone row.
const groups = {
  policy: ["rust", "crates/mcp-policy/src/**/*.rs"],
  runtime: ["rust", "crates/mcp-runtime/src/**/*.rs"],
  adapters: ["rust", "crates/mcp-starter-adapters/src/**/*.rs"],
  transport: ["rust", "crates/mcp-transport/src/**/*.rs"],
  project: ["rust", "crates/builder-project/src/**/*.rs"],
  application: ["rust", "crates/builder-app/src/**/*.rs"],
  generator: ["rust", "crates/builder-generator/src/**/*.rs"],
  playground: ["rust", "crates/builder-playground/src/**/*.rs"],
  cli: ["rust", "apps/cli/src/**/*.rs"],
  "desktop-native": [
    "rust",
    "apps/desktop/src-tauri/src/**/*.rs",
    "apps/desktop/src-tauri/build.rs",
  ],
  "desktop-ui": ["angular", "apps/desktop/ui/src/**/*.ts"],
  standalone: ["standalone", "crates/builder-generator/template/src/**/*.rs"],
};
const coreOnly = new Set([
  "crates/builder-app/src/smoke.rs",
  "apps/desktop/ui/src/main.ts",
  "apps/desktop/ui/src/app/smoke-ipc.ts",
  "apps/desktop/ui/src/app/workspace-smoke.component.ts",
]);

// These source files contain only declarations/re-exports. Report them separately from
// files with executable behavior and an absent compiler or feature mapping.
const declarations = new Set([
  "apps/desktop/src-tauri/src/commands.rs",
  "crates/builder-app/src/diagnostic.rs",
  "crates/builder-app/src/lib.rs",
  "crates/builder-playground/src/lib.rs",
  "crates/builder-project/src/scenario.rs",
  "crates/mcp-runtime/src/lib.rs",
  "crates/mcp-transport/src/lib.rs",
]);

const candidates = [];
for (const [component, [profile, ...patterns]] of Object.entries(groups)) {
  for (const rawPath of globSync(patterns, { cwd: root })) {
    const path = rawPath.replaceAll("\\", "/");
    if (/(?:\/tests(?:\/|\.)|\.spec\.ts$)/u.test(path)) continue;
    const key =
      profile === "standalone"
        ? path.slice("crates/builder-generator/template/".length)
        : profile === "angular"
          ? path.slice("apps/desktop/".length)
          : path;
    candidates.push({ component, profile, path, key, lines: new Map() });
  }
}
candidates.sort((a, b) => a.path.localeCompare(b.path, "en"));

// Record exact source spans for the test-only blocks currently used within Rust files.
// A new layout fails with a location instead of silently widening the exclusion.
function testLines(source, path) {
  const omitted = new Set();
  for (const [index, line] of source.entries()) {
    const cfg = /^(\s*)#\[cfg\((?:test|all\(test,.*\))\)\]$/u.exec(line);
    if (!cfg) continue;
    let end = index + 1;
    while (source[end]?.trimStart().startsWith("#[")) end++;
    if (source[end]?.endsWith("{")) {
      end = source.indexOf(cfg[1] + "}", end + 1);
    } else if (!/[,;]$/u.test(source[end] ?? "")) {
      end = -1;
    }
    assert.ok(end >= 0, `Unrecognized test-only span at ${path}:${index + 1}`);
    for (let row = index + 1; row <= end + 1; row++) omitted.add(row);
  }
  return omitted;
}

for (const [profile, files] of Object.entries(reports)) {
  for (const path of files) {
    let current;
    let matched = 0;
    for (const line of readFileSync(path, "utf8").split(/\r?\n/u)) {
      if (line.startsWith("SF:")) {
        const sourcePath = line.slice(3).replaceAll("\\", "/").toLowerCase();
        current = candidates.find((file) => {
          if (file.profile !== profile) return false;
          const key = file.key.toLowerCase();
          if (profile === "standalone") {
            const isRuntimeCopy =
              /(?:^|\/)crates\/(?:builder-project|mcp-[^/]+)\/src\//u.test(
                sourcePath,
              );
            return (
              !isRuntimeCopy &&
              (sourcePath === key || sourcePath.endsWith("/" + key))
            );
          }
          return sourcePath === key || sourcePath.endsWith("/" + key);
        });
        if (current) matched++;
      } else if (line === "end_of_record") {
        current = undefined;
      } else if (current && line.startsWith("DA:")) {
        const [number, hits] = line.slice(3).split(",", 2).map(Number);
        assert.ok(
          Number.isSafeInteger(number) && number > 0,
          `Invalid line counter in ${path}`,
        );
        assert.ok(
          Number.isSafeInteger(hits) && hits >= 0,
          `Invalid hit counter in ${path}`,
        );
        current.lines.set(
          number,
          Boolean(current.lines.get(number) || hits > 0),
        );
      }
    }
    assert.ok(
      matched > 0,
      `No authored ${profile} source recognized in ${path}`,
    );
  }
}

const counts = () => ({ covered: 0, total: 0 });
const scopes = () => ({ core: counts(), security: counts() });
const overall = scopes();
const components = Object.fromEntries(
  Object.keys(groups).map((name) => [name, scopes()]),
);
const languages = { rust: scopes(), typescript: scopes() };
const nativeAndFeatureFiles = {};
const noCounters = [];
const declarative = [];
for (const { component, profile, path, lines } of candidates) {
  if (declarations.has(path)) {
    declarative.push(path);
    assert.equal(
      lines.size,
      0,
      `Revisit declaration-only classification for ${path}`,
    );
    continue;
  }
  const source = readFileSync(join(root, path), "utf8").split(/\r?\n/u);
  if (profile !== "angular") {
    for (const number of testLines(source, path)) lines.delete(number);
  }
  if (!lines.size) {
    noCounters.push(path);
    continue;
  }
  assert.ok(
    [...lines.keys()].every((line) => line <= source.length),
    `Stale LCOV counters for ${path}`,
  );
  const result = {
    covered: [...lines.values()].filter(Boolean).length,
    total: lines.size,
  };
  if (
    /(?:\/windows(?:\/|\.)|\/linux\.|\/http(?:\/|\.)|\/stdio(?:\/|\.))/u.test(
      path,
    ) ||
    [
      "apps/desktop/src-tauri/src/paths.rs",
      "crates/builder-app/src/local_fs.rs",
    ].includes(path)
  ) {
    nativeAndFeatureFiles[path] = result;
  }
  const language = profile === "angular" ? "typescript" : "rust";
  for (const scope of [overall, components[component], languages[language]]) {
    for (const name of coreOnly.has(path) ? ["core"] : ["core", "security"]) {
      scope[name].covered += result.covered;
      scope[name].total += result.total;
    }
  }
}

function withPercent(scopes) {
  return Object.fromEntries(
    Object.entries(scopes).map(([name, value]) => [
      name,
      {
        ...value,
        percent: value.total
          ? Number(((100 * value.covered) / value.total).toFixed(2))
          : null,
      },
    ]),
  );
}

console.log(
  JSON.stringify(
    {
      overall: withPercent(overall),
      languages: Object.fromEntries(
        Object.entries(languages).map(([name, value]) => [
          name,
          withPercent(value),
        ]),
      ),
      components: Object.fromEntries(
        Object.entries(components).map(([name, value]) => [
          name,
          withPercent(value),
        ]),
      ),
      nativeAndFeatureFiles,
      noCounters,
      declarative,
      // Their protections are reviewed through runtime and rendered-UI checks. These do not
      // produce one LCOV counter per authored HTML, manifest or configuration line.
      structuralEvidence: [
        "apps/desktop/src-tauri/capabilities/main.json",
        "apps/desktop/src-tauri/tauri.conf.json",
        "apps/desktop/ui/src/**/*.html",
      ],
    },
    null,
    2,
  ),
);
