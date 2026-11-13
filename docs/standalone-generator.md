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

Choose a new destination in an existing local directory. Generation stages beside the destination and rechecks the source immediately before publishing. Generation leaves build, test and launch as separate actions. The generated README describes launching the binary with a reviewed operator config. For nonempty approvals, bind the config to the manifest's exact SHA-256 revision. The no-config launch starts with an empty grant and prompt set. The config and any environment values belong to the operator. [Export contract](contracts.md#standalone-output-and-regeneration) · [Runtime policy](contracts.md#policy-approvals-and-credentials)

## Regenerate an edited export

```powershell
mcp-builder generate .\project --out .\export-next --from-export .\export --preview --json
mcp-builder generate .\project --out .\export-next --from-export .\export --resolutions .\resolutions.json
```

The previous export stays usable. The generator carries its tracked project-owned files forward. Edited and removed generator-owned files appear in the preview, along with files added outside the previous generation record. Each resolution uses the preview's `path`, `observedDigest`, and `replacementDigest`. Use `keep` to publish the observed bytes under project ownership, or `regenerate` to use the current template. If a file has no template replacement, `regenerate` leaves it out of the new tree. Use JSON `null` for an absent digest. A changed observed file or changed replacement requires a new decision. Local runtime config, `.env` files, Git metadata and Cargo build output stay outside the regenerated export.

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
