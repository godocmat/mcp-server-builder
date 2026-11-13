# MCP Server Builder shared contracts

Phase 0 design · [Architecture](architecture.md) · [Acceptance traceability](traceability.md) · [Decision records](../DECISIONS.md)

F1 uses these boundaries to define shared types. Each owning task implements the slice named in the [backlog](../TASKS.md). The [MCP specifications and D1 wire matrix](mcp-compatibility.md) govern protocol messages. Builder contracts below govern portable data, native commands and evidence.

## Portable project

A project directory contains one UTF-8 JSON `mcp-project.json` and explicitly referenced assets. Import accepts a project directory or, in v1, a ZIP with the manifest at its root. ZIP export contains the manifest and approved project assets. Generation produces a separate Rust tree. Validate before saving, importing, launching or generating. Use diagnostics with a field pointer and capability ID when available. [Project requirements](requirements.md#functional-requirements-and-acceptance-checks)

| Field | Shape and validation |
| --- | --- |
| `format`, `schemaVersion` | `"mcp-server-builder.project"` and an integer. The pilot writes 1. Commercial v1 reads 1 and 2 and writes 2 for new projects. The version selects the parser and migration chain before interpreting the rest of the document. |
| `projectId`, `name`, `server` | UUID, project display name, and `{name, version, description?, instructions?}`. Use a SemVer string for the server version. Renaming the project display name preserves the project UUID, root and server identity. |
| `transport` | Schema 1 uses `"stdio"`. Schema 2 uses `{kind:"stdio"}` or `{kind:"streamableHttp", endpointPath}`. The path defaults to `/mcp` on create, must be an absolute URL path and has no query or fragment. Both use the two-revision contract below. Deploy-specific host, issuer, private bind address and principal grants are operator settings. |
| `capabilities`, `bindings`, `policy` | Arrays of the tagged records below. `policy` is `{grants: Grant[]}`. Empty grants deny adapter access. Each reference resolves in the project and matches the expected binding or capability kind. |
| `secretRefs` | Array of `{id, purpose}` declarations. An integration or environment declaration refers to the ID. Values and resolution sources are operator-owned. |
| `scenarios` | Array of `ScenarioV1` records. Fixtures and expectations are explicit project content under the [scenario contract](#scenarios-results-and-evidence). |
| `environment`, `dependencies` | Schema 2 adds required arrays. An environment entry is `{name, source:{kind:"literal", value} \| {kind:"secret", ref}}`. Literals are public config. A custom Rust dependency is `{name, version, features: Key[]}` with an exact registry version and a resolved lock for generation. Reject Git and external path dependencies in the first custom profile. Runtime/toolchain packages come from the generator's pinned template, with no implicit dependency additions at launch. |

`Key` is an ASCII identifier matching `[a-z][a-z0-9_-]{0,63}`. IDs are stable within their collection. Tool and prompt names are unique within their kind, and resource URIs and templates must resolve unambiguously. A project asset path uses relative slash-separated segments beneath the project root. Reject absolute paths, empty and traversal segments, platform alias collisions and filesystem objects outside the approved storage boundary. A binding for data outside the project uses a named target resolved locally or at deployment.

Validate tagged fields, sizes, UTF-8, unique object keys, uniqueness of IDs, referential integrity and supported settings. Reject unknown fields in a known document version. On unsupported versions, report the readable format and version and preserve the original bytes for recovery. Use JSON Schema 2020-12 for declared tool schemas with in-document references. Compile and validate without fetching external schema documents. Tool inputs and declared structured outputs use object-root schemas in the portable dual-revision profile.

### Capability and binding records

All capability records have `id`, `kind`, `name` and `description`. A referenced binding also has an `id` and a discriminator `kind`. Each adapter accepts the fields defined for that discriminator and a declared operation. Tool input never selects an executable, query, credential, origin or root.

| Capability `kind` | Additional fields |
| --- | --- |
| `tool` | `inputSchema`, optional `outputSchema`, `bindingId`, optional `timeoutMs`. The runtime validates arguments before dispatch and declared successful structured output before publication. |
| `resource` | `uri`, `mimeType`, `bindingId`, optional `timeoutMs`. The registered URI resolves to the configured read binding. |
| `resourceTemplate` | `uriTemplate`, `variables` (map of template variable names to local schemas), `mimeType`, `bindingId`, optional `timeoutMs`. Start with simple named URI-template variables. Match a registered template and validate expanded variables before using them. A file variable must resolve to a safe path segment in the approved root. |
| `prompt` | `arguments: [{name, description?, required}]` and `messages: [{role, textTemplate}]`. Arguments are strings and message roles are `user` or `assistant`. Substitute declared variables as text, with bounded expansion. |

| Binding `kind` | Required configuration and enforcement |
| --- | --- |
| `rest` | `origin`, `method`, `pathTemplate`, `pathParams` (placeholder to input JSON Pointer), optional `bodyPointer`, and optional `headersFromSecrets` (header name to secret reference). Resolve against one granted scheme, host and port, encode path values as segments, and verify method, path and actual peer. Live credential headers require HTTPS. Deny redirects by default. The starter adapter checks any separately permitted redirect for a bodyless read at the original origin and method. |
| `fileRead` | `rootRef` and `relativePathTemplate`. The root is operator-mapped. Read an allowed file via a verified root-relative handle. Reject links, special files and unsupported filesystem forms under the initial profile. |
| `sqlite`, `postgres` | `targetRef`, `operation` (`read` or `write`), fixed `statement`, and ordered `params` with `{pointer, type}` for typed values. For PostgreSQL include `credentialRef`. Use least-privilege roles or approved file handles and enforce bound rows, bytes and time. Writes require an explicit grant and store privileges. |
| `git` | `repositoryRef`, `operation` (`showCommit` or `readDiff`), and `revisionPointers` for the validated object IDs. Scope reads to the approved local repository. Disable implicit network access and external helpers. |
| `customRust` | Project-relative `source` and declared `entry` identifier. Build and launch as reviewed trusted code under the invoking identity. Its effects can exceed managed adapter grants. |
| `executable` | `executableRef`, `cwdRef`, an `args` vector of literals or `{pointer, constraint}` entries, and `envNames` allowlist. Bind operator-selected executable and directory identities, validate each argument and launch directly under the tested supervision boundary. |

An MVP manifest uses the stdio transport, the REST and file-read bindings, and templated prompts. This complete minimal schema 1 example is valid as a document. Its requested host grant and target would still need operator approval before live execution.

```json
{
  "format": "mcp-server-builder.project",
  "schemaVersion": 1,
  "projectId": "8d1ea3a7-18c4-4e36-8a8c-6e84af79eed0",
  "name": "Catalog sample",
  "server": {"name": "catalog", "version": "0.1.0"},
  "transport": "stdio",
  "capabilities": [
    {
      "id": "lookup_item",
      "kind": "tool",
      "name": "lookup_item",
      "description": "Find a catalog item",
      "inputSchema": {
        "type": "object",
        "properties": {"id": {"type": "string"}},
        "required": ["id"],
        "additionalProperties": false
      },
      "outputSchema": {"type": "object"},
      "bindingId": "catalog_lookup"
    }
  ],
  "bindings": [
    {
      "id": "catalog_lookup",
      "kind": "rest",
      "origin": "https://catalog.example.test",
      "method": "GET",
      "pathTemplate": "/items/{id}",
      "pathParams": {"id": "/id"}
    }
  ],
  "policy": {
    "grants": [
      {
        "id": "catalog_read",
        "capabilityId": "lookup_item",
        "operation": "http.read",
        "target": {"kind": "rest", "origin": "https://catalog.example.test", "method": "GET", "pathTemplate": "/items/{id}"}
      }
    ]
  },
  "secretRefs": [],
  "scenarios": []
}
```

### Policy, approvals and credentials

- A `Grant` is `{id, capabilityId, operation, target, limits?}`. `operation` is one of `http.read`, `http.write`, `file.read`, `db.read`, `db.write`, `git.read`, `exec.run` or `custom.invoke`. The tagged `target` narrows the corresponding binding as shown below. `limits` may lower `timeoutMs` and `maxOutputBytes`. Deny any operation or resolved target outside the intersection. Prompts render declared text through the registry and remain subject to caller policy and limits. [Adapter boundaries](security.md#adapter-access)
- Managed local actions require a matching approval. Review resolves the executable and source identity, argument vector, working directory, environment names and references, policy and concrete target bindings, operation and limits. An approval binds the local registration or profile, OS operator, operation, canonical review digest and relevant content digests. Build review also binds the source tree, dependency lock and toolchain. Connection review binds the endpoint and allowed trust settings. OAuth review binds the discovered issuer and resource. Re-evaluate the review digest and verify identities at use. A change in covered inputs or grant revocation invalidates reuse. One-use approvals are the pilot default. V2 adds explicit remembered approvals with expiry and revocation. A one-use approval is consumed on admission to its reviewed operation, including an unsuccessful launch.
- Launch, connect, build and dependency locking have distinct approval scopes. Saving, validating, migrating or rendering source resolves no credential values and runs no imported code. Import classifies inline credentials as references needing rebinding and discards their values. Project JSON and exported client configs carry references or login settings. Target path and profile mapping changes trigger normal review and validation.
- A scoped credential resolver accepts a resolved principal or trusted local execution context, binding ID and secret reference. It returns a value only to the authorized native consumer or process. Use one explicitly chosen source per binding. MVP offers session-only and explicit environment injection. V2 adds Windows Credential Manager, macOS Keychain and Linux Secret Service, with actionable locked or unavailable status and explicit session or environment rebinding. A deployed runtime resolves configured restricted secret files or explicit environment references under its dedicated identity. No credential value travels in command arguments. IPC and normal CLI output carry reference names and status. OAuth access tokens are scoped to the remote client or resource-server boundary and never sent to an integration as its credential. [Credential and evidence checks](security.md#tm-005-credentials-and-evidence)
- Local target mappings key on the local registration and a portable reference. They resolve an approved root, repository, database or executable to an operator-selected native target. Target selection is inert until reviewed for use. An operator can bind a project-relative asset or an explicitly selected external path. A PostgreSQL target binds a reviewed endpoint and database with a separate credential reference.

| Grant target `kind` | Narrowing fields |
| --- | --- |
| `rest` | `origin`, `method`, `pathTemplate`, optional `allowedPeerRanges` for internal peers. Redirects are off by default. An enabled redirect must match an explicit grant and pass the same peer check. |
| `fileRead` | `rootRef`, optional narrower `relativePathTemplate`. |
| `sqlite`, `postgres` | `targetRef`, `bindingId` of the reviewed statement. |
| `git` | `repositoryRef`, approved read `operation`. |
| `executable` | `executableRef`, `cwdRef`, `bindingId` of the reviewed argument and environment constraints. |
| `customRust` | `source`, `entry` of the reviewed handler. The grant controls dispatch into trusted code. The OS identity bounds the handler's ambient privileges. |

A standalone server reads an operator-owned runtime config with `configVersion: 1`, `kind` matching its transport, `approvedGrantIds`, `approvedPromptIds`, `targetBindings`, `secretSources` and optional tighter `limits`. Grants and prompts default to empty. Any approved grant or prompt requires `manifestRevision`, the SHA-256 revision of the exact reviewed manifest bytes. `targetBindings` maps portable references to reviewed targets. `secretSources` maps references to `{kind:"env", name}` or, for deployment, `{kind:"file", path}`. The generated README documents preparing the runtime config from a references-only example and launching with it. Managed preview gets the equivalent approved inputs through a private launch handoff.

The v1 `streamableHttp` variant adds `bindAddress`, `publicResource`, `issuer`, `allowedHosts`, `allowedOrigins` and `principalGrants`. A principal rule maps verified `{issuer, subject}` and optionally a verified client identifier to allowed grant and capability IDs and required scopes. Access is default-deny. Discovery filters capabilities to the caller's permitted visibility. Validate mandatory settings before readiness. [Authenticated ingress](deployment.md#tls-and-ingress)

## Local persistence and migration

The manifest is the source of truth for portable project data. The per-user SQLx SQLite catalog in the OS application-data directory owns project registrations, target mappings, connection profiles, credential binding metadata, approvals, run results and local audit. GUI and CLI use the same per-user profile by default. Use foreign keys, transactions for related catalog writes, WAL on supported local filesystems and a bounded busy timeout. Keep database and evidence files private to the owning user. Support portable project folders on the verified local filesystems.

| Operation | Contract |
| --- | --- |
| Open or list | Open directly from an operator-selected root. Return a local registration ID, normalized project view and manifest revision token. The token is `sha256:<hex>` over the exact on-disk bytes. Catalog entries can be stale. Reconcile them from the current manifest on open and surface missing or inaccessible projects. |
| Create, save and recover | New projects get a UUID. Serialize builder writes per canonical project root with an OS-backed lock and compare `expectedRevision` under the lock. Validate first, stage and flush a new manifest on the same volume, recheck for external edits and use the tested native replace primitive. Retain a last known-good prior revision and a pre-migration original. A failed comparison returns `project.revision_conflict` with the current revision and leaves the caller's draft available for saving as a copy. Recover a prior valid revision with explicit selection and the same compare-and-replace check, or open it as a separate copy. Report recoverable paths and revisions after interrupted writes. |
| Catalog failure | Commit the manifest first, then reconcile the index in a catalog transaction. If that transaction fails after the file commit, return the committed revision and an index repair warning. A retry uses the returned revision. Catalog migration runs in an exclusive migration window after a consistent SQLite backup. Roll back a failed transaction and offer direct open or recovery of project files. Never resolve a conflict by writing a catalog snapshot over a manifest. |
| Preview import | Read bounded inert data and report format, source digest, proposed file list, diagnostics, credential rebindings and destination checks. For ZIP, reject traversal, absolute paths, normalized collisions, links, special files and entries exceeding size or expansion limits before committing project files. Confirm the same digest and destination at commit, stage under a private sibling directory and publish a complete project into a new root. No implicit install, launch, build, network discovery or secret lookup. |
| Duplicate and export | Duplicate to a new root and assign a new project UUID and local registration. Use staged publication for archive export. Copy explicitly referenced safe assets with source and destination checks. Export excludes local registration and approvals, histories, credential source mappings, process state and captured live payloads. |
| Migrate | Select the ordered migration chain for the supported schema. The pilot-to-v1 step changes `"transport":"stdio"` to `{"kind":"stdio"}` and adds empty `environment` and `dependencies` arrays. Preview shows source and target versions, resulting document and diagnostics with no write. Commit rechecks the exact old revision, retains the original, validates the result and publishes with the normal save protocol. V1 can continue reading and editing schema 1 within its feature set. A schema 2-only edit requires the explicit migration. Preserve data after failure and refuse unsupported older or newer versions with a recovery diagnostic. |

M1 verifies revision conflicts, interrupted replace, catalog reconciliation and unsupported versions. V1 migrates an actual schema 1 pilot fixture, injects failure before publication and proves the original reopens. R3 repeats on the advertised filesystems, including case and Unicode behavior. [Import threat verification](security.md#tm-001-import-and-preview) · [Recovery matrix](traceability.md#local-mvp)

## Standalone output and regeneration

The generator consumes a validated snapshot plus template, runtime and dependency-lock inputs. It emits a Cargo workspace, lockfile, project manifest, references-only operator configuration example, entry point and handlers, unit and wire integration tests, README, and version and redistribution notices. V1 adds examples for additional operator configuration and the Linux deployment inputs. Standard template variants carry a tested lock. A changed custom dependency graph requires an explicitly reviewed lock operation that produces a matching lock before final export. Validation and generation are offline file operations. A build is a separately approved execution of the emitted project.

The portable `generation.json` format is `{version:1, projectId, projectSchemaVersion, snapshotDigest, generatorVersion, runtimeVersion, templateVersion, protocolVersions, cargoLockDigest, files}`. `files` lists `{path, owner:"generator"|"project", digest}` for published files except the record itself. Hashes use SHA-256. The normalized snapshot digest uses the generator's versioned deterministic serialization of typed inputs and referenced asset digests. Generated contents omit timestamps, absolute builder paths and other machine-specific metadata. Sort output paths and serialization keys consistently.

Create output in a new destination through a staged sibling and publish only after completion. Regeneration reads the previous generation record, computes file digests and stages a new destination. Carry forward tracked project-owned sources. Edited or removed generator-owned files and untracked additions require a per-file keep or regenerate decision tied to the observed and replacement digests before publication. A kept edit becomes project-owned in the new record. Detect destination collisions and recheck the input snapshot before publication. An interrupted generation leaves the previous export usable and a retry can publish to a clean destination. M6 proves deterministic contents, edit preservation and a standalone locked build. V9 repeats with the full template and transport set. [M6 generator workflow](standalone-generator.md) · [Runtime distribution decision](../DECISIONS.md#adr-006--standalone-generation-and-license-continuity)

## MCP transport and execution

Both revisions use the same immutable catalog, authorization policy, schema checks and adapter dispatch. Advertise tools, resources and prompts when supported and visible. Support list and call, list and read including expanded URI templates, and list and get respectively. Use stable catalog ordering and opaque cursors scoped to the catalog and caller. Provide bounded progress for long calls with a supplied progress token. Modern discovery, lists and resource reads start with `ttlMs: 0` and `cacheScope: "private"`. Modern ordinary results use `resultType: "complete"`. Return a text representation alongside successful structured tool output. [Capability baseline](mcp-compatibility.md#capability-baseline)

| Profile | Lifecycle and cancellation |
| --- | --- |
| `2026-07-28` stdio | Newline-delimited UTF-8 JSON-RPC. Accept `server/discover` or a direct request with `io.modelcontextprotocol/protocolVersion` and `io.modelcontextprotocol/clientCapabilities` in `params._meta`. Use stdout for protocol and stderr for sanitized diagnostics. `notifications/cancelled` targets the active request ID on the connection. EOF triggers bounded shutdown. |
| `2025-11-25` stdio | `initialize`, negotiated capabilities, then `notifications/initialized` and legacy operations including `ping`. Offer only the selected legacy revision on the initialize path. A peer unable to speak it disconnects. Handle `notifications/cancelled` for in-flight calls outside initialization. |
| `2026-07-28` HTTP | POST each request with per-request metadata. Validate `MCP-Protocol-Version`, `Mcp-Method` and applicable `Mcp-Name` against the parsed body before dispatch, including specified decoding. Return immediate JSON or a request-scoped SSE response. Use SSE for calls awaiting adapter I/O so disconnect cancels the associated work. Ignore legacy session and resumption headers on modern requests. The modern-only fixture returns 405 for GET and DELETE. |
| `2025-11-25` HTTP | Initialize and issue a bounded in-memory session. Require the negotiated version and issued ID on subsequent requests. Bind the session to the verified principal and client for authenticated deployments and authorize every use. A subsequent legacy request without a session ID returns 400. Ended or foreign IDs return 404. Accept legacy notifications with empty HTTP 202. A legacy cancellation POST stops the named request. A dropped legacy stream leaves the request running up to its deadline. Support authenticated DELETE for the session owner. Return 405 for standalone GET and provide no resumption. |

Use a non-mutating discovery probe in auto mode. On stdio, fall back to legacy initialization on an unrecognized probe failure or probe timeout as defined by the transport. On HTTP, fall back on an unrecognized or empty HTTP 400 on the probe. A recognized modern version, header or capability error stays on the modern error path, and HTTP 401 enters authentication. Legacy session expiry and restart require a new initialization. The compatibility profile covers 2025-11-25 Streamable HTTP. Separate HTTP+SSE, headerless fallback to 2025-03-26, dynamic catalog updates, resource subscriptions, server-initiated sampling, roots and elicitation, protocol logging, completion, Skills, Tasks and MCP Apps are deferred. The builder's own audit, traces and cancellation stay available. [Modern HTTP binding](https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/streamable-http) · [Legacy transports](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports)

Authoring custom HTTP parameter-header annotations is deferred. Reject `x-mcp-header` in portable tool definitions. The modern HTTP playground client validates and mirrors supported annotations from external definitions as required by the binding. V4 verifies valid mirroring and rejects invalid definitions in a controlled fixture. [Modern parameter-header rules](https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/streamable-http#custom-headers-from-tool-parameters)

Authenticate remote MCP requests before dispatch. Verify configured issuer, access-token type, signature and allowed algorithm, exact resource audience, expiry and validity window. Match scopes and principal grants for the requested capability and concrete action. Use explicit Host and Origin policies and trust forwarding only from the configured ingress. Validate a supplied Origin and accept absence for native clients. Publish the RFC 9728 path-specific resource metadata and a Bearer challenge pointing to it. M4 verifies the binding on loopback. V3 and R1 repeat the protected paths through the reference provider and ingress. [Provider and trust verification](security.md#tm-007-http-ingress-and-resource-server)

### Cancellation, deadlines and limits

An execution receives a cancellation token, monotonic deadline and correlation ID with its approved principal and target context. Index active stdio work by connection and JSON-RPC ID, legacy HTTP work by bound session and ID, and modern HTTP work by its request stream. A cancellation, deadline, authorization-window end or shutdown signals only the relevant work. Ignore unknown or completed IDs. Finish exactly one local terminal result and audit outcome, and suppress late responses on closed streams. Adapters close HTTP bodies and file handles, interrupt and roll back open database transactions where possible, and terminate and reap owned per-call processes. Custom Rust is cooperative within the documented trusted-code boundary. Record a committed effect or an unknown outcome when cancellation races with an effect. Replay a live mutation only after checking its outcome and confirming an explicit safe retry or idempotency contract. [Cancellation and adapter evidence](mcp-compatibility.md#errors-timeouts-and-cancellation) · [Native supervision](security.md#tm-004-local-launch)

The initial bounds below are test inputs. M3/M4, V3 and R1 measure and lock the applicable defaults against the reference workload. An approved deployment may tighten them.

| Boundary | Initial bound and check |
| --- | --- |
| Managed calls | Default 10 s, hard cap 30 s, including when progress is sent. Bound a request body or stdio message to 1 MiB and a serialized response to 1 MiB. M3/M4 verify oversized data stops before dispatch or publication. |
| Remote admission | Start with 32 simultaneous calls per service and 4 per principal, plus a caller limit of 60 requests/minute with burst 10. Start ingress at 120 requests/minute per source with burst 20. Preserve a bounded authenticated control path for cancellation and session termination when work quotas are full. V3/R1 verify 413 for body limit and 429 for admission quotas, no rejected work reaches adapters, and cancellation still operates. |
| HTTP sessions and streams | Start with 128 legacy sessions and 15 minutes idle expiry. Expire idle sessions and drop all sessions on restart. Bound protected streams by the earliest of deadline, token expiry and shutdown. Use a 35 s ingress idle timeout initially and disable response buffering and retries. V3/R1 verify timeouts, renewal of authorization for new calls and restart. |
| Process and service shutdown | Local stop closes stdin, allows up to 2 s graceful exit, then terminates and reaps within a total 5 s. Linux service drops readiness and stops admission, drains up to 10 s, then cancels, reaps and flushes bounded audit within a 15 s total application deadline. Start with 20 s under systemd or Compose. M7/M10 and R1/R3 verify their respective bounds. |

## Connection profiles

A `ConnectionProfile` is `{id, revision, name, protocol, endpoint}` stored locally. `protocol` is `auto` by default or an explicit supported revision. `endpoint` is one of these tagged variants.

- `{kind:"managed", registrationId}` identifies the builder's managed preview.
- `{kind:"stdio", executable, args, cwd, environment}` records an inert parsed command and declared environment names or references. Review resolves the command to an absolute executable under the operator's account and displays the working directory and declared bindings before spawn.
- `{kind:"http", url, auth}` describes an HTTPS MCP endpoint. `auth` has kind `oauth` with a client registration profile, or `bearerEnv` with an explicit secret binding for operator-supplied authentication. An unauthenticated loopback variant exists only for controlled engineering fixtures.

V1 imports Claude Code `.mcp.json` entries under `mcpServers` for stdio and Streamable HTTP. Preview accepted entries and report unsupported or ambiguous fields per entry. Preserve executable and argument boundaries. Interpret supported environment references as references and show how to rebind inline credentials. Opening an import creates inert records after review. Connection, issuer discovery and spawn happen only through their approved operations. Builder projects with editable handlers use the portable project import.

For native OAuth, start with a pre-registered public client, system browser, PKCE S256, one-time state and a five-minute loopback callback window. The first reference registration uses `http://127.0.0.1:8768/oauth/callback`. Allow another explicitly registered local port if the operator selects it. Bind the listener before browser launch. Validate exact resource and issuer from metadata, and exact callback state and any returned issuer. Require callback issuer when the server advertises it. Recheck changed endpoints or issuers before sending credentials. Send the canonical `resource` in authorization, exchange and refresh. Serialize rotating refresh per issuer, resource, client and account. Offer scope step-up after an explicit review of the requested scopes. Credential output reports only binding status. Forgetting a profile closes its owned connections, removes local credential bindings and revokes its local approvals. Pre-registration is the reference path. CIMD and any restricted DCR fallback follow the evidence gates in the decision record. [OAuth decision and client matrix](../DECISIONS.md#adr-007--external-oauth-and-client-profiles)

Client config export accepts a client profile, reviewed transport inputs and a new destination. It emits a snippet or instructions with the resolved executable and argument vector or public MCP URL, scope and login hints, and supported environment references. Claude Code uses `.mcp.json`, Codex CLI uses a `config.toml` entry, OpenCode V2 uses its `mcp.servers` shape, and generic instructions cover Inspector and other MCP clients. Use the target format's native reference mechanism where verified and placeholder names with setup instructions otherwise. Existing client configuration is untouched. V10 validates emitted files and the actual sign-in and wire behavior in isolated client profiles. [D1 versioned client details](mcp-compatibility.md#client-matrix)

## Application services and Tauri IPC

The shared Rust services implement the operation semantics in this table. Tauri exposes the named commands through the main local window only. Each command accepts one typed JSON `request` object and returns `Reply<T>`. F1 owns the common Rust DTOs and TypeScript binding check. Generate or verify TypeScript types against the Rust DTOs in CI, using strict discriminated unions and camelCase JSON keys. Never deserialize Rust storage rows, SDK DTOs or arbitrary native commands from the WebView. CLI inputs call the same Rust services with paths and approval inputs supplied by the OS operator. [Native entry point verification](security.md#tm-002-privileged-entry-points)

The native picker returns a short-lived, purpose-bound `PathSelection` for a user-selected project root, import file, destination, executable or working directory. An IPC command accepts that handle or a local registration ID. Registered project assets use validated relative paths. Rust owns path resolution and rechecks it before I/O. A review handle is local to its initiating session. Reusable approvals have their own IDs and are validated on each use.

| Service | Tauri command | Request → successful data | Milestone |
| --- | --- | --- | --- |
| Native picker | `select_path` | `{purpose}` → `{selectionId, displayPath}` or `null` on cancel | MVP |
| Project | `project_list`, `project_open` | `{}` → `ProjectSummary[]`; `{root: PathSelectionOrRegistration}` → `ProjectSnapshot` | MVP |
| Project | `project_create` | `{destination: PathSelection, name}` → `ProjectSnapshot` | MVP |
| Project | `project_validate`, `project_save` | `{document: ProjectDocument}` → `{valid:true}`; `{registrationId, expectedRevision, document}` → `ProjectSnapshot` | MVP |
| Project | `project_recover` | `{registrationId, expectedRevision, source: {kind:"backup", revision} \| {kind:"draft", document}, destination?: PathSelection}` → `ProjectSnapshot` | MVP |
| Project | `project_rename`, `project_duplicate` | `{registrationId, expectedRevision, name}`; `{registrationId, expectedRevision, name, destination: PathSelection}` → `ProjectSnapshot` | v1 |
| Import | `import_preview`, `import_commit` | `{source: PathSelection, kind:"project"\|"clientConfig", destination?: PathSelection}` → `ImportPreview`; `{previewId, sourceDigest, selectedEntryIds?}` → `{kind:"project", snapshot} \| {kind:"clientConfig", profiles: ConnectionProfile[]}` | v1 |
| Migration | `migration_preview`, `migration_commit` | `{registrationId, expectedRevision}` → `{sourceRevision, fromVersion, toVersion, document}`; same input → `ProjectSnapshot` | v1 |
| Export | `project_export` | `{registrationId, expectedRevision, destination: PathSelection}` → `ExportReceipt` for a project ZIP | v1 |
| Generation | `generation_plan`, `generation_write` | `{registrationId, expectedRevision, destination: PathSelection, fromExport?: PathSelection}` → `GenerationPlan`; `{planId, planDigest, resolutions?: ConflictResolution[]}` → `ExportReceipt` | MVP |
| Build | `dependency_lock`, `build_export` | `{subject: ReviewSubject, approval: ApprovalInput}` → `{operationId}`; observe through `run_poll` | v1 |
| Target | `target_list`, `target_bind`, `target_remove` | `{registrationId}` → `TargetBindingSummary[]`; `{registrationId, ref, target: TargetBindingInput}` → `TargetBindingSummary`; `{registrationId, ref}` → `{removed:true}` | File roots in MVP, other local targets in v1 |
| Policy | `policy_review`, `policy_approve` | `{subject: ReviewSubject}` → `PolicyReview`; `{reviewId, digest, remember?}` → `{approvalId, expiresAt}` | MVP, remember in v1 |
| Policy | `approval_list`, `approval_revoke` | `{owner: LocalOwner}` → `ApprovalSummary[]`; `{approvalId}` → `{revoked:true}` | v1 |
| Connection | `connection_save`, `connection_list`, `connection_forget` | `{profile: ConnectionDraft, expectedRevision?}` → `ConnectionProfile`; `{}` → `ConnectionProfile[]`; `{id, expectedRevision}` → `{removed:true}` | v1 |
| Credential | `credential_status`, `credential_bind`, `credential_remove` | `{owner: LocalOwner}` → `CredentialStatus[]`; `{owner, ref, source: CredentialInput}` → `CredentialStatus`; `{owner, ref}` → `{removed:true}` | Session and env in MVP, OS store in v1 |
| OAuth | `oauth_login`, `oauth_logout` | `{profileId, expectedRevision, approval: ApprovalInput}` → `{operationId}`; `{profileId, accountId?}` → `{removed:true}` | v1 |
| Playground | `playground_start`, `playground_stop` | `{subject: ReviewSubject, approval: ApprovalInput}` → `{operationId}`; `{connectionId}` → `{stopped:true}` after bounded teardown | MVP managed stdio, v1 profiles |
| Playground | `playground_discover`, `playground_call` | `{connectionId}` → `Discovery`; `{connectionId, call: Invocation}` → `{runId}` | MVP |
| Scenario | `scenario_run` | `{registrationId, expectedRevision, scenarioId, connectionId, allowLive?}` → `{runId}`. Edit scenarios through project save. | MVP fixture, v1 remote |
| Operations | `run_poll`, `run_cancel` | `{id, afterCursor?}` → `{events: OperationEvent[], nextCursor, terminal}`; `{id}` → `{cancellationRequested}` | MVP |
| Evidence | `report_export`, `audit_query` | `{runIds: UUID[], destination: PathSelection, includeTrace?}` → `ExportReceipt`; `{owner: LocalOwner, afterCursor?, limit?}` → `{events: AuditEventV1[], nextCursor?}` | v1 |
| Client export | `client_config_export` | `{client:"claudeCode"\|"codexCli"\|"openCodeV2"\|"generic", target: ClientExportTarget, destination: PathSelection}` → `ExportReceipt` | v1 |

- `ProjectSnapshot` is `{registrationId, rootDisplayPath, revision, document}`. `ProjectSummary` uses the registration, name, last known revision and status. `ImportPreview` includes `previewId`, `sourceDigest`, candidate format, entry IDs and summaries, and diagnostics. `project_recover` requires a new destination for a draft copy and a matching current revision for a backup replace.
- `GenerationPlan` is `{planId, planDigest, files, conflicts, versions}`. A conflict lists relative path and compared digests. `ConflictResolution` is `{path, reviewedDigest, action:"keep"|"regenerate"}`. `ExportReceipt` carries output path, format version and digest, and for generation, its version record. Both write APIs revalidate the planned inputs and destination.
- `TargetBindingInput` is tagged as a native path selection, a validated project-relative asset, or a PostgreSQL endpoint and database. Its target type must match the referenced binding. `TargetBindingSummary` shows the reference, target type and resolved display metadata. Launch review binds the mapping version and resolved identity.
- `ReviewSubject` is a tagged union for `launch` (managed registration or stdio profile plus revision), `connect` (HTTP profile and revision), `oauth` (profile, revision and discovered issuer/resource), `lockDependencies` (registration, manifest and source snapshot), and `build` (selected export and source snapshot). Rust computes resolved launch, build or endpoint details. `PolicyReview` contains `reviewId`, `digest`, the resolved sanitized review summary and typed decisions. `ApprovalInput` carries an `approvalId` in Tauri. CLI supplies a reviewed digest or a reusable ID. Each operation accepts only its corresponding review subject. Review of imported endpoints itself stays offline. When a reviewed connection discovers a new issuer, complete the OAuth review before browser login or token exchange.
- `LocalOwner` is `{kind:"project"|"connection", id}`, using local registration or profile ID. `ConnectionDraft` carries name, protocol and the inert endpoint fields. `ApprovalSummary` contains owner, operation, digest, expiry and revocation state. `CredentialStatus` contains owner, reference, selected source and availability. `CredentialInput` is a tagged explicit environment name, session value or OS-store value. The secret entry command is write-only, bound to the local window and the selected reference. It clears the entry state after use and returns status only.
- `Discovery` contains server identity, selected revision, visible capabilities and supported methods. `Invocation` selects `tools/call`, `resources/read` or `prompts/get` with its typed name or URI and arguments. `OperationEvent` carries a cursor, operation ID, timestamp and one of progress, sanitized trace, transient sanitized result, terminal `RunResultV1`, operation-specific completion data or failure diagnostic. A successful playground start returns a `connectionId` and discovery data in the completion event. Polling is bounded and reports expired cursors. Cancellation is idempotent for known operations, including a terminal run. No long-running IPC call must hold the editor open.
- `ClientExportTarget` is reviewed stdio launch inputs or the public HTTPS URL, protocol profile, scopes and supported authentication references. It is inert configuration data. The export neither starts the target nor initiates sign-in.

## CLI command specification

The executable is `mcp-builder`. The operands below use project roots and file paths, and local IDs for profiles, credentials and results. Commands keep stdout for the selected result format and stderr for sanitized progress. All commands except the stdio bridge accept `--json` for one versioned object. Interactive CLI review shows the same resolved plan as the desktop. Headless execution requires the reviewed digest via `--approve <digest>` or a matching reusable `--approval-id <id>`. The command recomputes the plan at use. A headless command missing approval returns the structured policy denial without waiting for input. [CLI acceptance](traceability.md#local-mvp)

| Command and operands | Behavior | Milestone |
| --- | --- | --- |
| `project list`; `project create <root> --name <name>`; `project open <root>` | List, initialize, or read a portable project and return its current revision. | MVP |
| `project validate <root>`; `project save <root> --from <manifest> --expect <revision>` | Use the shared field diagnostics and revision-checked save. | MVP |
| `project recover <root> --from-revision <backup> --expect <revision>`; `project recover <root> --draft <manifest> --out <new-root>` | Restore a chosen backup or preserve a draft as a new project. | MVP |
| `generate <root> --out <new-root> [--from-export <previous-root>]` | Plan and stage standalone source. `--preview` returns the plan without publishing. Regeneration accepts an explicit `--resolutions <file>` for reviewed conflicts. | MVP, full adapters in v1 |
| `target list <root>`; `target bind <root> <ref> --path <path>`; `target remove <root> <ref>` | Manage scoped native target mappings. V1 adds `--postgres <endpoint> --database <name>` for a PostgreSQL target. | File roots in MVP, other targets in v1 |
| `policy review <launch\|connect\|oauth\|lock-dependencies\|build> <target>` | Print the resolved plan and digest. A target is a project root, profile ID, or export root appropriate to the action. | Launch in MVP, remaining actions in v1 |
| `test run <root> --scenario <id>` or `--all` | Replay saved expectations with fixture mode by default. Use the launch approval option or, in v1, `--connection <id>` with the corresponding approval. Require `--allow-live` for a live scenario. V1 accepts `--report-out <file>` and optional `--include-trace` to export before closing the connection. | MVP, remote in v1 |
| `run start <root-or-stdio-profile> --stdio` | Foreground supervised MCP stdio bridge for a real client. Protocol on stdout and sanitized diagnostics on stderr. EOF or an interrupt requests bounded teardown. This command owns the supervised process for its lifetime. | v1 |
| `project rename <root> --name <name> --expect <revision>`; `project duplicate <root> --out <new-root> --name <name> --expect <revision>` | Update display name or create a distinct portable project. | v1 |
| `project import <source> --out <new-root> --preview`; `project import <source> --out <new-root> --expect-source <digest>`; `project export <root> --out <zip> --expect <revision>` | Review and commit import with source digest, or stage a portable archive. | v1 |
| `project migrate <root> --expect <revision> --preview` or `--apply` | Preview or commit the same deterministic migration. | v1 |
| `connection import <.mcp.json> --preview` or `--expect-source <digest>`; `connection list`; `connection save --from <profile> [--expect <revision>]`; `connection forget <id> --expect <revision>` | Inert import or edit of local connection profiles. Import can select individual entries. | v1 |
| `connection inspect <id>`; `auth login <id>`; `auth logout <id>` | Discover through an approved connection and close it on completion, or sign in through an approved native browser flow and manage the local token binding. | v1 |
| `credential status <owner>`; `credential bind <owner> <ref> --env <name>` or `--store-stdin`; `credential remove <owner> <ref>` | Display reference status, choose an environment variable or write a credential from stdin, or remove the binding. For a command with free stdin, `--secret-stdin <ref>` supplies a session-only value for that invocation. | Status and env in MVP, OS store in v1 |
| `policy approve <action> <target> --approve <digest> --remember`; `policy list <owner>`; `policy revoke <id>` | Create, list or revoke a reusable approval. | v1 |
| `dependencies lock <root>`; `build <export-root>` | Explicitly review and approve dependency resolution or a build. Use the bound source and dependency inputs in a scoped build environment. | v1 |
| `report export <run-id> --out <report.json>`; `audit list <owner>`; `client export <profile> --target <config> --out <file>` | Write sanitized evidence or client configuration. The client profile enum is the same as in IPC. | v1 |

`<owner>` is `project:<registration-id>` or `connection:<profile-id>`. Each privileged command accepts the approval options. On an approved connection, discovery can save the candidate issuer and resource against the profile revision and return `auth.required`. Review that metadata before `auth login`, which requires browser interaction. A remote headless run can use a previously established credential binding or an explicitly supported bearer environment profile. An absent binding reports how to authorize the profile. The stdio bridge carries JSON-RPC, and its setup failures appear on stderr with a documented exit code. M9 verifies the pilot commands from a fresh shell. V12 verifies v1 CLI/GUI parity and machine-readable output. [CLI test strategy](testing.md#gui-cli-and-standalone-runs)

## Diagnostics and errors

`Reply<T>` is `{ok:true, data:T, diagnostics: Diagnostic[]}` or `{ok:false, error:Diagnostic, diagnostics: Diagnostic[]}`. CLI JSON adds top-level `version:1` and a stable `command` name to this envelope. A `Diagnostic` is `{code, message, severity, correlationId, action?, field?, capabilityId?, details?}`. Use a stable dot-delimited code, severity `error` or `warning`, human-readable text, an actionable next step where known, and a JSON Pointer for a field. Details are typed per code and allowlisted. Examples include the current revision for a save conflict, a denied rule ID, a missing credential reference, supported versions, or a failed run ID. Diagnostic text and optional fields may evolve compatibly. Consumers branch on the code. Validation returns `project.invalid` with ordered field diagnostics on failure. Never include native exception chains, credentials or raw payloads in public diagnostics.

| CLI exit | Stable category and sample codes |
| --- | --- |
| `0` | Success, including warnings such as `project.index_repair_required`. |
| `1` | Unexpected application failure, such as `internal.error`. |
| `2` | Usage or invalid input/configuration, such as `cli.usage`, `project.invalid`, `project.unsupported_schema` or `input.invalid`. |
| `3` | Permission, trust or authorization denial, such as `policy.denied`, `policy.approval_required`, `auth.required` or `auth.insufficient_scope`. |
| `4` | Test assertion mismatch, `scenario.assertion_failed`. |
| `5` | Build, child process, transport, upstream or execution failure, such as `process.spawn_failed`, `process.unexpected_exit`, `transport.disconnected`, `runtime.timeout`, `mcp.tool_error` or `adapter.failed`. |
| `6` | Revision conflict, recovery, local I/O or missing/unavailable credential, such as `project.revision_conflict`, `export.conflict`, `storage.unavailable` or `credential.missing`. |
| `130` | Operator cancellation or interrupt, `run.cancelled`. |

Report a suite's first failing scenario in stable manifest order for exit mapping, and retain the evaluated runs for report export. An expected denial, error or timeout asserted successfully by a scenario counts as a pass. Cancellation of the suite takes precedence. JSON usage errors use the same envelope when `--json` is present. A failed run carries its ID in typed details so CI can retrieve a sanitized report.

MCP errors have their own wire mapping. JSON-RPC parse, envelope and method failures use the specified protocol codes and preserve a valid request ID where applicable. An unknown tool or malformed invocation envelope uses `-32602`. A known tool's schema failure, policy denial or adapter failure uses a sanitized tool result with `isError:true`. A missing resource uses `-32602` on the modern revision and `-32002` on the legacy revision. An unknown prompt or missing required prompt argument uses `-32602`. Requests for capabilities hidden by principal policy follow the unknown-target response for that method. Other authenticated policy denials carry a sanitized policy reason appropriate to the capability. HTTP auth uses 401 with resource metadata for missing or invalid tokens, and 403 with the scope challenge for insufficient scope. Invalid Origin also yields 403. The modern wire maps missing or mismatched routing headers to HTTP 400 / `-32020`, required client capability failures to 400 / `-32021`, unsupported protocol version to 400 / `-32022`, and unknown RPC method to 404 / `-32601`. [D1 revision-specific error cases](mcp-compatibility.md#errors-timeouts-and-cancellation)

On managed and generated servers, attach a sanitized `{code, correlationId}` at `result._meta["mcp-server-builder/diagnostic"]` for tool errors. Where the protocol error schema permits application details, use the same key under `error.data`. Standard clients receive the ordinary result or protocol error and readable text. The playground maps that metadata back to the builder's typed codes for its own runtimes. For unrelated servers, use generic diagnostic codes such as `mcp.tool_error` with the available sanitized protocol details. M3/M4 verify the metadata round trip and M9 checks the CLI denial exit through a managed stdio run. [MCP metadata rules](https://modelcontextprotocol.io/specification/2025-11-25/basic#_meta)

## Scenarios, results and evidence

All records in this section use camelCase UTF-8 JSON with `version:1`. Treat imported fixtures, expected values and external responses as untrusted text/data. A scenario stores only deliberate inputs and synthetic fixtures. Saving a scenario from a run does not copy live payloads by default.

| Record | Fields and semantics |
| --- | --- |
| `ScenarioV1` | `{version, id, name, mode:"fixture"\|"live", call: Invocation, fixtures?, assertions, sensitivePaths?}`. Tool arguments are a JSON object. Resource reads take a registered or expanded URI. Prompt arguments are named strings. A fixture-mode scenario supplies controlled responses keyed by adapter binding where needed. A live run requires explicit operator selection and the target's normal approval and authorization. |
| Fixture | Tagged per adapter and binding ID. The starter `rest` fixture maps a concrete approved method/path to a bounded synthetic HTTP status and body. `fileRead` maps a validated relative path to bounded synthetic text. Fixture execution uses the shared policy path and controlled fixture targets without resolving production credentials. V5–V9 add disposable test data or process fixtures for new adapters. Missing fixtures fail the scenario with no live fallback. Malformed wire testing is an explicit controlled fixture operation, with bounded input and a separate test process or endpoint. |
| Assertion | `{kind:"equals", path, expected}`, `{kind:"schema", path, schema}` or `{kind:"errorCode", code}`. `path` is a JSON Pointer over the normalized successful result. Equality compares JSON values with object order ignored. Error assertions use the normalized diagnostic code. Mismatch diagnostics identify the path. The transient, sanitized diff can show bounded expected and actual values. |
| `RunResultV1` | `{version, runId, correlationId, scenarioId?, target, transport, protocolRevision?, startedAt, durationMs, outcome, effect, assertions, diagnostic?, traceSummary?}`. Use UTC RFC 3339 times and monotonic durations. `outcome` is `passed`, `assertionFailed`, `invalid`, `denied`, `failed`, `timedOut` or `cancelled`. `effect` is `none`, `committed` or `unknown`. Persist result status and assertion indices, paths and codes. |
| Trace entry | `{sequence, offsetMs, direction, kind, method?, messageId?, correlationId, payload?, truncated}`. `direction` is `sent` or `received`, with `stderr` for local process diagnostics. Keep ordered, sanitized messages and duration on the request timeline. Record any dropped-entry count and truncation in `traceSummary`. |
| `AuditEventV1` | `{version, eventId, timestamp, correlationId, principal, operation, capabilityId?, decision, reason, outcome}`. `principal` is the local OS operator identity or a normalized verified issuer/subject and client identity. Pre-auth denials use an anonymous principal. `decision` is `allow`, `deny` or `notEvaluated`. Record a stable reason code and sanitized execution outcome. The event carries metadata only. |
| Report | `{format:"mcp-server-builder.report", version:1, generatedAt, builderVersion, runs, summary, traces?}`. `runs` contains the selected sanitized `RunResultV1` records. The summary counts terminal outcomes. Optional trace lists key by run ID and include truncation and omitted counts. Missing trace data has an explicit status. Export writes one JSON file with a digest. |

The normalized successful tool result has `content` and optional `structuredContent`. Resource results have `contents`, and prompt results have `messages`. Wire-only metadata stays out of assertion paths. Evaluate assertions on the bounded result before redacting the displayed values and diagnostics. For example, the schema 1 tool above can use this portable scenario.

```json
{
  "version": 1,
  "id": "known_item",
  "name": "Known catalog item",
  "mode": "fixture",
  "call": {"method": "tools/call", "name": "lookup_item", "arguments": {"id": "42"}},
  "fixtures": [
    {
      "kind": "rest",
      "bindingId": "catalog_lookup",
      "method": "GET",
      "path": "/items/42",
      "status": 200,
      "body": {"id": "42", "name": "Sample item"}
    }
  ],
  "assertions": [
    {"kind": "equals", "path": "/structuredContent/name", "expected": "Sample item"}
  ]
}
```

Redact in Rust before any WebView response, diagnostic, trace, log, store or export. Omit credential headers, cookies, OAuth exchange and callback data, raw session IDs and non-allowlisted audit fields. Mask known secret values and designated sensitive paths, including in assertion mismatches, upstream failures and buffered stderr. Default to metadata-only persistence. Opt-in trace payload views are bounded to 500 entries or 2 MiB per connection, 8 KiB per entry, and discarded at connection close unless explicitly exported while available. Record truncation. Keep local run and audit metadata for up to 30 days or 100 MiB per profile, whichever is reached first. A generated service writes sanitized structured audit JSON lines to a private rotated state location with the same initial age and size caps per service. Operational logs go to journald or the container log driver with configured retention. V2 owns redaction and local retention tests. R1 verifies service evidence across restart and rotation, and R3 repeats the synthetic canary and retention checks on release inputs. [Security evidence design](security.md#local-workflows-and-credentials) · [Deployment recovery](deployment.md#persistence-and-operations-drill)
