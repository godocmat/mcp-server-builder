# M1 local project model

[Portable project contract](contracts.md#portable-project) · [Persistence contract](contracts.md#local-persistence-and-migration) · [Acceptance traceability](traceability.md#local-mvp)

`builder-project` owns the schema 1 document and pure validation. `parse_manifest` selects the reader from `format` and `schemaVersion`, checks the JSON shape and returns a `ValidatedManifest` with the typed document and exact bytes. `validate_document` serializes and checks a typed draft for save. Both reject duplicate keys, malformed fields, unknown settings and inconsistent references. Tool schemas declare an object root. Tool, variable and assertion schemas compile with the offline JSON Schema 2020-12 compiler. REST origins, references, prompts and fixtures are inert inputs. A scenario can intentionally submit arguments that fail the declared schema.

The pilot supports stdio, REST and file-read bindings, and the four capability kinds. REST and file templates accept one simple `{name}` placeholder per path segment with optional affixes. Resource URI templates place distinct variables in path segments. Prompt text uses `{name}` and doubled braces for literal braces. File paths use slash-separated portable segments. A file fixture is `{kind:"fileRead", bindingId, relativePath, text}`. Validation caps manifest bytes at 4 MiB, JSON nesting at 64 levels, parsed values at 100,000 and each main record collection at 512 entries. It reports fixed diagnostic messages with JSON Pointers and valid capability IDs where available. Optional fields use omission.

## Native services and storage

- Construct `ProjectService::for_user()` in the native shell. The profile directory is `com.mcpserverbuilder.desktop` under the OS local application-data directory. `with_profile` accepts an absolute native path to a dedicated alternate directory. On Unix the profile directory requires owner-only permissions. Windows uses the ACL inherited from the per-user profile. SQLx uses bundled SQLite, foreign keys, WAL and a two-second busy timeout. The catalog stores registration metadata and the last observed revision.
- Call `open`, `create`, `save`, `list` and `recover` from a Tokio runtime. Resolve trusted picker handles at the shell boundary before constructing a `ProjectLocation::Root`. `ProjectLocation::Registration` resolves through the catalog. A registration is stable for a canonical root and portable UUID. Copies in different folders have independent registrations. Replacing the document in a registered folder with a different UUID requires opening by folder, which creates the new registration.
- `project_validate`, `project_read_draft` and `project_validate_root` check input without indexing it. `project_validate_document` checks a typed editor draft. Use the byte-based APIs when reading a file so duplicate keys and version information reach the validator. `open` validates the manifest and indexes valid data. Open and validation leave the project directory read-only.
- Builder writes serialize on `.mcp-project.lock` in the canonical root. They hash the live bytes under the lock, stage and flush beside the manifest, retain the prior valid bytes in `.mcp-project.last-valid.json` and recheck the live revision immediately before publication. Windows publishes with `MoveFileExW` using a write-through move on the same volume. Unix uses a same-directory rename and syncs the directory. A missing manifest is created with an atomic no-clobber operation. The lock covers the subsequent index update. A writer that ignores the builder lock can still race the final check and native replace. Use operator-owned local project folders. [Windows file move API](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-movefileexw) · [SQLite WAL](https://www.sqlite.org/wal.html)
- `project.revision_conflict` includes `details.currentRevision`. Recovery identifies valid backup or interrupted-stage files by path and exact revision. `ProjectRecovery::Backup` restores a selected last-valid revision when the expected revision still matches. An absent expected revision requires a missing manifest. `ProjectRecovery::CopyBackup` and `ProjectRecovery::Draft` preserve content at another destination under a new project UUID. An interrupted stage can be loaded as a draft and saved to a new folder. The service preserves an unsupported manifest even when a backup is available.
- `project.index_repair_required` is a success warning with the committed revision. Retry by opening the folder. The catalog rechecks the live revision inside its indexing transaction to prevent a slow open from overwriting a newer index entry. An unavailable catalog still permits direct path operations. Initializing an existing version-zero catalog takes an exclusive builder migration lock, makes a consistent SQLite backup with `VACUUM INTO`, and applies the schema in a transaction. An unsupported catalog version or a failed transaction leaves direct project access available. [SQLite backup command](https://www.sqlite.org/lang_vacuum.html#vacuum_with_an_into_clause)

## Handoff

Build the M3 registry from the validated `ProjectDocument`. `builder_project::compile_schema` compiles declared schemas offline. `ProjectDocument.policy` is an `mcp_policy::RequestedPolicy` that feeds the shared evaluator directly with separately reviewed operator and caller grants. Project validation applies M2's grant checks and enforces schema 1 target kinds, binding references, path narrowing and limit bounds. The typed policy normalizes HTTP origins and CIDRs for the editor view. The source bytes and their revision remain exact until an explicit save. The [schema 1 catalog fixture](../crates/builder-project/tests/fixtures/catalog-v1.json) exercises both starter bindings, all four capability kinds, requested grants, a credential reference and scenarios. Use it as a schema 1 source for V1 migration.

M8 and M9 own shell requests, picker handling, reply rendering and CLI exit mapping. V1 owns explicit portable schema 1 → 2 migration and its pre-migration original. F2 and R3 own native Linux and macOS and the wider filesystem evidence. M1's save evidence comes from Windows 11 Pro 25H2 x64 build `26200.9974` on NTFS. Power-loss testing and other filesystem combinations remain release evidence.

## Windows verification

Use the F1 Rust/Cargo `1.99.1` and MSVC tools from the worktree root. Override the F1 target path to keep this checkout's outputs isolated. [F1 setup](development.md#windows-prerequisites-and-local-setup)

```powershell
. (Join-Path $env:LOCALAPPDATA 'McpBuilderF1\env.ps1')
$env:CARGO_TARGET_DIR = Join-Path (Get-Location).Path 'target'
cargo fmt --all --check
cargo test --locked --workspace --exclude mcp-builder-desktop
cargo clippy --locked --workspace --exclude mcp-builder-desktop --all-targets -- -D warnings
cargo build --locked -p mcp-builder
$cli = Join-Path $env:CARGO_TARGET_DIR 'debug\mcp-builder.exe'
& $cli smoke
& $cli smoke --json
& $cli --json smoke --contract-version 2
git diff --check
```

The non-desktop workspace passed 26 tests. The fixtures exercised byte-for-byte round trips, malformed and future versions, invalid fields and references, credential canaries, zero connections to a configured loopback origin, native cross-process locking, competing saves, an external edit during staging, a Windows sharing violation at replacement, last-valid recovery, catalog rollback and repair, and consistent SQLite backup with pending WAL data. The existing CLI human and JSON smoke both exited `0`. An unsupported smoke contract exited `2` with `input.invalid` at `/contractVersion`.

## Integrated verification

Independent verification of the integrated baseline `1dba1e9` on Windows passed Rust formatting, both locked workspace builds, all 44 locked workspace tests, all-target workspace Clippy with warnings denied, the CLI smoke and all six locked SDK profiles. The embedded build used SHA-256-verified browser assets from the F2 and M2 integration with the same Angular sources and lockfile. [Foundation checks](ci.md#reproduce-the-checks)

On the same baseline, Ubuntu 24.04.3 amd64 userspace under WSL2 passed 43 tests across the project, application and policy crates, including the native save and recovery cases, and all-target Clippy with warnings denied.

```sh
cargo test --locked -p builder-app -p builder-project -p mcp-policy
cargo clippy --locked -p builder-app -p builder-project -p mcp-policy --all-targets -- -D warnings
```

The first GitHub-hosted matrix run is pending a repository remote. [Hosted verification](ci.md#first-hosted-run)

The schema 1 fixture builds an M2 policy directly from its typed requests. The regression exercises the allowed REST call and its narrower grant limits, the allowed file read and a denial outside that file path. Additional checks preserve exact source bytes across origin normalization and report invalid policy fields with their JSON Pointers.
