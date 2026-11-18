# M6 standalone generator

`mcp-builder generate` reads a validated schema 1 project and emits a standalone Cargo workspace. The output includes the normalized manifest, an editable `src/handlers.rs`, shared runtime and adapter sources, the stdio transport, a locked dependency graph, an operator config example, and unit and wire tests. `generation.json` records the project, template, runtime and protocol versions and the SHA-256 digests of the lockfile and emitted files. The snapshot digest uses versioned serialization of the typed manifest with sorted object keys. Schema 1 maps file roots through operator configuration.

## Generate and build

```powershell
mcp-builder generate .\project --out .\export --preview --json
mcp-builder generate .\project --out .\export
Set-Location .\export
cargo build --locked
cargo test --locked
```

Choose a new destination in an existing directory whose contents and permissions you control. Generation stages beside the destination and rechecks the source immediately before publishing. Windows publication uses local NTFS. Linux publication requires `openat2`, procfs and a local filesystem with no-replace directory rename support. Other platforms return a storage diagnostic before writing. The generated README describes building, testing and launching separately with a reviewed operator config. For nonempty approvals, bind the config to the manifest's exact SHA-256 revision. The no-config launch starts with an empty grant and prompt set. The config and any environment values belong to the operator. [Export contract](contracts.md#standalone-output-and-regeneration) · [Runtime policy](contracts.md#policy-approvals-and-credentials)

## Regenerate an edited export

```powershell
mcp-builder generate .\project --out .\export-next --from-export .\export --preview --json
mcp-builder generate .\project --out .\export-next --from-export .\export --resolutions .\resolutions.json
```

The previous export stays usable. The generator carries its tracked project-owned files forward. Edited and removed generator-owned files appear in the preview, along with files added outside the previous generation record. Each resolution uses the preview's `path`, `observedDigest`, and `replacementDigest`. Use `keep` to publish the observed bytes under project ownership, or `regenerate` to use the current template. If a file has no template replacement, `regenerate` leaves it out of the new tree. Use JSON `null` for an absent digest. A changed observed file or changed replacement requires a new decision. Top-level local runtime config, `.env` files, Git metadata and Cargo build output stay outside the regenerated export. Keep any additional operator-only files outside the prior export or discard them at conflict review.

```json
{
  "version": 1,
  "resolutions": [
    {
      "path": "notes.md",
      "observedDigest": "sha256:<copy observedDigest from preview>",
      "replacementDigest": null,
      "decision": "keep"
    }
  ]
}
```

The CLI's JSON envelope reports the file list or conflicts without showing file contents. A blocked publish returns exit `6` with `export.conflict`. Invalid or stale resolution files return exit `2` with `export.invalid_resolution`. The shared application service exposes the same plan and outcomes for the desktop workflow.

## Licensing

Each export carries the Apache-2.0 license and attribution for the builder's
bundled runtime and original scaffolding under `licenses/mcp-server-builder/`.
Project manifests, data, and project-authored source follow the terms their
owners choose. Keep the builder license and attribution alongside your own
license when redistributing an export. Check the included
`THIRD-PARTY-NOTICES.md` against the dependencies for your target before
distributing binaries.

Regenerating from an earlier export adds the builder license and attribution.
If the earlier export has an untracked project license, use `keep` on that file
during conflict review to carry it into the new export.

## Verification

On Windows 11 Pro x64 build 26200.9974 with Rust 1.99.1, two CLI exports from the catalog fixture matched across all 61 recorded files and `generation.json`. The CLI returned field diagnostics for invalid input, preserved an edited handler, blocked an unresolved and a stale file decision, regenerated the reviewed README, and excluded the previous operator config. The generator's native tests exercised a late destination collision and a failed source recheck followed by a successful retry.

A CLI-generated workspace in a separate directory passed locked offline build, formatting, all-target Clippy and 23 workspace tests. The four raw stdio cases exercised both MCP revisions, default-deny discovery, and rejection of approval reuse after a manifest change. Inspector CLI 2.15.0 on Node 24.22.3 launched the standalone Windows binary over stdio. Both revisions listed and invoked tools, read the fixed and templated resources, and listed and rendered a prompt. The local upstream observed exactly two approved requests. [MCP testing matrix](testing.md#wire-and-client-interoperability)

The production generator's eight publication tests and four export integration tests passed on each of tmpfs and ext4 in an offline x64 QEMU guest running the Ubuntu 24.04 kernel `6.8.0-147-generic`. The test binaries used Rust 1.99.1 and the locked static musl build. Linux standalone build and wire verification remain pending. macOS publication is unsupported.

The foundation workflow generates, builds and tests the export on its Windows and Linux workers. [CI commands](ci.md#reproduce-the-checks) · [Runtime distribution decision](../DECISIONS.md#adr-006--standalone-generation-and-license-continuity)
