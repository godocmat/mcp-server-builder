# Standalone MCP server

This export bundles a Rust stdio server, the project's schema 1 manifest, and
the runtime sources as local Cargo dependencies. The MCP client launches one
process per connection. The server supports MCP 2026-07-28 and 2025-11-25 and
uses stdin and stdout for JSON-RPC. Startup diagnostics use stderr.

## Build and test

Use Rust and Cargo 1.99.1, pinned in `rust-toolchain.toml`. On Windows x64, use
an MSVC developer shell with the C++ build tools and Windows SDK. The REST
adapter's TLS backend builds native AWS-LC code. Use a C toolchain for the host
and NASM on Windows x64, or opt in to the locked AWS-LC crate's prebuilt NASM
objects with `AWS_LC_SYS_PREBUILT_NASM=1` in the build environment.

Review the source and lock before building. Cargo may need to fetch the pinned
toolchain and registry dependencies on first use. From this directory, run:

```text
cargo build --locked
cargo test --locked
```

The tests use disposable local data and exercise the running stdio executable.
The builder and its account services are unnecessary for build and launch.

## Review the manifest and configure access

The default policy denies every tool, resource, resource template, and prompt.
To enable selected capabilities, copy `runtime-config.example.json` to a private
operator-owned file, such as `runtime-config.local.json`.

- Review the selected manifest, including its bindings, requested grants, and
  prompt text. Copy the `mcp-project.json` file digest from `generation.json`
  into `manifestRevision` after confirming the manifest still matches that
  digest. The digest is `sha256:` followed by 64 lowercase hexadecimal
  characters. To review an edited manifest, compute SHA-256 over its exact
  bytes with `Get-FileHash -Algorithm SHA256 ./mcp-project.json` in PowerShell
  or `sha256sum mcp-project.json` on Linux. Revisit the approvals before using
  a new digest.
- Set `approvedGrantIds` to the reviewed IDs from `policy.grants`. For REST,
  the approved grant bounds origin, method, path, peer addresses, and any
  grant-specific limits. Private or loopback peers need explicit
  `allowedPeerRanges` in the grant. Redirects are disabled.
- Set `approvedPromptIds` to the reviewed prompt capability IDs.
- For each approved file-read grant, map its `rootRef` in `targetBindings`
  to an operator-controlled absolute directory. For example, the mapping for
  `catalog_docs` can be `{"kind":"fileRead","path":"C:/reviewed/catalog"}`.
  Windows file roots require local NTFS with exact path spelling. Linux file
  roots require local ext4 or tmpfs, `openat2`, and procfs. macOS file roots require
  local APFS and exact stored names. The server rejects
  unsupported file-root mappings at startup.
- For any HTTPS credential binding, map its manifest secret reference in
  `secretSources` to a named environment source. For example, `catalog_auth`
  can map to `{"kind":"env","name":"CATALOG_TOKEN"}`. Set the environment
  variable in the launching client's environment. The runtime looks up the
  named variable after it authorizes the request and authenticates the HTTPS
  peer. Keep values in the launch environment and protect the private
  configuration from edits by other users.
- Optionally use `"limits":{"timeoutMs":1000,"maxOutputBytes":16384}` to
  tighten request limits. Each supplied limit must be a positive integer.

## Launch from an MCP client

Set the executable to the absolute path of `target/debug/mcp-standalone-server`
or `target/debug/mcp-standalone-server.exe` on Windows. Pass the following
arguments as separate entries, substituting the actual paths:

```text
--manifest /path/to/mcp-project.json --config /path/to/runtime-config.local.json
```

For a default-deny smoke run, omit `--config`. If you omit `--manifest`,
the server reads `mcp-project.json` from the client's chosen working
directory. Choose regular files in trusted directories for both inputs.
The server refuses symbolic links and reparse points at the final path
component. Pass any required environment variables through the client.

`runtime_config.manifest_revision_required` means approval IDs need a reviewed
manifest digest. `runtime_config.manifest_revision_mismatch` means the digest
differs from the selected file. For `runtime_config.target_binding_required`,
check the root references of the approved file grants. For
`adapter.file_root_unavailable`, check the absolute directory spelling,
access rights, and filesystem profile.

## Customize and regenerate

`src/handlers.rs` assembles the validated manifest, shared policy, and starter
adapters. Regeneration carries this project-owned source forward. Edit
portable capabilities in the manifest or edit the handler assembly as
trusted Rust code, then review, rebuild and test. `generation.json` records
file ownership and hashes, along with the generator, template, runtime and
protocol versions. Keep the operator configuration outside the portable
project and review it for each manifest revision.

## Licensing

MCP Server Builder’s bundled runtime and original scaffolding use Apache-2.0.
Project manifests, data, and project-authored code follow the terms chosen by
their owners. For redistribution, choose the terms for your project and include
the builder license, attribution, and applicable dependency notices. The
bundled Cargo packages declare their own Apache-2.0 license.
[Builder license](licenses/mcp-server-builder/LICENSE) ·
[Builder attribution](licenses/mcp-server-builder/NOTICE) ·
[Third-party notices](THIRD-PARTY-NOTICES.md)
