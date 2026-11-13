import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";

const host = execFileSync("rustc", ["-vV"], { encoding: "utf8" }).match(/^host: (.+)$/m)?.[1];
const nativeBackend = {
  "x86_64-pc-windows-msvc": "schannel",
  "aarch64-apple-darwin": "security-framework",
  "x86_64-unknown-linux-gnu": "openssl",
}[host];
assert.ok(nativeBackend, `Expected an ADR-008 build host, got ${host}`);

const profiles = [
  ["server-stdio", ["server", "transport-io"]],
  ["server-http", ["server", "transport-streamable-http-server", "transport-streamable-http-server-session"]],
  ["server-stdio,server-http", ["server", "transport-io", "transport-streamable-http-server", "transport-streamable-http-server-session"]],
  ["client-stdio", ["client", "transport-child-process"]],
  ["client-http-rustls", ["client", "auth", "transport-streamable-http-client-reqwest", "reqwest"], "rustls"],
  ["client-http-native-tls", ["client", "auth", "transport-streamable-http-client-reqwest", "reqwest-native-tls"], "native-tls"],
];
const evidencePackages = new Set(["rmcp", "reqwest", "rustls", "rustls-platform-verifier", "aws-lc-sys", "native-tls", nativeBackend]);

for (const [profile, requiredFeatures, tls] of profiles) {
  const selection = ["--locked", "-p", "mcp-transport", "--no-default-features", "--features", profile];
  console.log(`\n${host} · cargo build ${selection.join(" ")}`);
  execFileSync("cargo", ["build", ...selection], { stdio: "inherit" });

  const graph = execFileSync("cargo", [
    "tree", ...selection, "--target", host, "--edges", "normal", "--prefix", "none", "--format", "{p}|{f}",
  ], { encoding: "utf8" });
  const packages = new Map();
  for (const line of graph.trim().split(/\r?\n/)) {
    if (line.endsWith(" (*)")) continue;
    const [pkg, features] = line.split("|");
    const name = pkg.split(" ", 1)[0];
    if (evidencePackages.has(name)) {
      console.log(`  ${pkg}: ${features}`);
      if (!packages.has(name)) packages.set(name, new Set());
      for (const feature of features.split(",")) packages.get(name).add(feature);
    }
  }

  for (const feature of requiredFeatures) {
    assert.ok(packages.get("rmcp")?.has(feature), `${profile} is missing rmcp/${feature}`);
  }
  assert.equal(Boolean(packages.get("rmcp")?.has("reqwest")), tls === "rustls", `${profile} rmcp/reqwest`);
  assert.equal(Boolean(packages.get("rmcp")?.has("reqwest-native-tls")), tls === "native-tls", `${profile} rmcp/reqwest-native-tls`);
  assert.equal(packages.has("reqwest"), Boolean(tls), `${profile} reqwest activation`);
  assert.equal(Boolean(packages.get("reqwest")?.has("rustls")), tls === "rustls", `${profile} reqwest/rustls`);
  assert.equal(Boolean(packages.get("reqwest")?.has("native-tls")), tls === "native-tls", `${profile} reqwest/native-tls`);
  assert.equal(packages.has("native-tls"), tls === "native-tls", `${profile} native-tls crate`);
  if (tls === "rustls") {
    for (const dependency of ["rustls", "rustls-platform-verifier", "aws-lc-sys"]) {
      assert.ok(packages.has(dependency), `${profile} is missing ${dependency}`);
    }
  }
  if (tls === "native-tls") assert.ok(packages.has(nativeBackend), `${profile} is missing ${nativeBackend}`);
}

console.log(`\nAll six locked SDK profiles passed on ${host}.`);
