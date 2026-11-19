# Local MVP CLI

The builder CLI opens the same schema 1 project as the desktop editor. A project folder contains `mcp-project.json`. Put the first-party `mcp-builder` executable on PATH or invoke it by its full path. Managed fixture replay uses a first-party stdio worker on Windows, macOS, and Linux. [Reference project](requirements.md#reference-project) · [Managed playground](local-playground.md)

## Open, edit, and export

```powershell
mcp-builder project create .\catalog --name 'Catalog sample'
mcp-builder project list
mcp-builder project open .\catalog --json
mcp-builder project validate .\catalog --json
mcp-builder generate .\catalog --out .\source --preview --json
mcp-builder generate .\catalog --out .\source --json
```

To edit from a script, save the current document as a separate draft, keep its project UUID, and set `--expect` to the revision returned by `project open`. If another save wins, the command returns `project.revision_conflict` with the current revision. Use `--draft` to carry your edit into a new project, or review the last valid backup in the recovery diagnostic and restore it against the current revision. If the live manifest is missing, pass `--expect-missing` in place of `--expect`. The default catalog is shared with the desktop. `--profile <directory>` selects a separate catalog for scripted project commands. [Project storage](local-project-model.md) · [Export and regeneration](standalone-generator.md)

```powershell
mcp-builder project save .\catalog --from .\draft.json --expect $currentRevision --json
mcp-builder project recover .\catalog --draft .\draft.json --out .\recovered --json
mcp-builder project recover .\catalog --from-revision $backupRevision --expect $currentRevision --json
```

## Review and run saved fixtures

```powershell
mcp-builder policy review launch .\catalog --scenario known_item --grant catalog_read --json
mcp-builder test run .\catalog --scenario known_item --grant catalog_read --approve $reviewedDigest --json
```

Read the selected grants, resolved executable and working directory, and manifest revision before copying `data.digest` into `$reviewedDigest`. Use the same grant and prompt selections for review and replay. A missing or stale digest returns exit 3. The worker starts with the reviewed fixture inputs and uses no upstream connections or credential sources. Use `--protocol modern` or `--protocol legacy` to pin a revision. With a single scenario, `--trace-payloads --json` includes sanitized payloads in the bounded trace. The earlier `playground review` and `playground replay` commands accept the same single-scenario inputs.

```powershell
mcp-builder policy review launch .\catalog --all --grant catalog_read --grant note_read --prompt review_item --json
mcp-builder test run .\catalog --all --grant catalog_read --grant note_read --prompt review_item --approve $reviewedSuiteDigest --json
```

`--all` selects fixture scenarios in manifest order. Review the listed scenarios and approve the outer `data.digest`, which binds the exact review for each scenario. Every admitted preview closes before the next starts. The suite returns sanitized run records in `data.runs` on success or `error.details.runs` on failure. The first failing scenario determines the exit category and its run ID appears in `error.details.runId` when a run was evaluated. Cancellation takes precedence. A denial that satisfies a saved error assertion counts as a pass.

| Exit | Script behavior |
| --- | --- |
| 0 | The command succeeded. Inspect `diagnostics` for warnings. |
| 1 | An internal application operation failed. |
| 2 | Usage or input is invalid. Inspect the reported field pointers. |
| 3 | Policy denied an operation or the launch needs current approval. |
| 4 | An assertion failed. Inspect the failed assertion index and visible path. |
| 5 | The process, transport, or fixture execution failed. |
| 6 | A revision, recovery, storage, or credential condition needs attention. |
| 130 | The operator cancelled the replay. |

Each JSON command emits one version 1 envelope with its stable command name and an `ok` discriminator. Progress goes to stderr. CLI replay covers saved fixtures. Live local target mappings and credential sources are configured per review in the desktop preview workflow. [CLI and diagnostic contracts](contracts.md#cli-command-specification)
