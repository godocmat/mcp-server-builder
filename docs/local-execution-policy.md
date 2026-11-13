# M2 local execution policy

The policy crate consumes portable access requests, reviewed operator grants and verified runtime facts. The same evaluator serves managed preview and generated servers. The caller supplies the applicable immutable policy snapshot for each evaluation. [Portable and runtime contracts](contracts.md#policy-approvals-and-credentials) · [Security boundaries](security.md#policy-enforcement)

## M1 project integration

- Use the Serde types `RequestedPolicy`, `Grant`, `GrantTarget`, `Operation` and `LimitOverrides` for the manifest policy. The schema 1 catalog policy round-trips through these types. The policy field requires a `grants` array. Optional fields can be omitted. Wrong JSON shapes, unknown fields and zero limit overrides fail parsing with `policy.invalid_shape`. Parse the raw document as a typed object to detect duplicate JSON fields. If M1 lands with its own pilot DTOs, M3 can project those inert requests into these types when building the registry.
- Call `RequestedPolicy::validate` for semantic validation. `PolicyInputError` carries a fixed code and a JSON Pointer relative to the policy, such as `/grants/0/operation`. Prefix it with `/policy` for a manifest diagnostic. M1 resolves capability and binding references, checks their kinds and operation compatibility, and validates the rest of the manifest. A parsed or validated `Grant` remains a request.
- Persist and review operator inputs through local application services or generated runtime configuration. Construct each `ApprovedGrant` from the grant snapshot bound to the reviewed project, target mapping and approval identity. An operator can narrow the snapshot. For `approvedGrantIds`, verify the active reviewed snapshot and resolve each ID against it before building `OperatorPolicy`. Snapshot identity, approval consumption and revocation live at the service boundary.

## M3 runtime integration

1. Validate the catalog and the incoming call, including declared argument schemas. Supply `Principal::Local` from the approved stdio launch or `Principal::Remote` from verified issuer, subject, optional client identity, scopes and token expiry. Build `CallerGrant` records from the operator's launch or deployment policy. A matching client-specific rule takes precedence over a subject-wide rule, including when it denies. `Principal::Anonymous` cannot see or invoke a protected capability.
2. Use `Policy::new` with the project request and operator snapshots. Reject a semantic error during setup. Use `Policy::is_visible` with the registered `CapabilityAction` to filter discovery. The visibility check uses capability and grant IDs, operation and required scopes. It consumes no target facts and issues no permit. For prompts, set the approved registered prompt IDs in the operator snapshot and the visible IDs in the caller rule. `Action::RenderPrompt` then uses the same authorization and output limits.
3. For adapter dispatch, take the operation from the registered binding. Evaluate `OperationContext` with the verified concrete target before credential resolution or effects. Carry the original `admitted_at` and the latest `Permit::ceiling()` through point-of-use rechecks against active policy. Use the earliest deadline for cancellation and preserve one cumulative byte budget across chunks. Apply a lower cap to that budget with `ByteBudget::tighten`. `ExecutionLimits::input_budget()` supplies the 1 MiB request cap for framing. The capability timeout defaults to 10 s and caps at 30 s. Project, operator, caller and request overrides only tighten the timeout or the 1 MiB output cap. Overlapping matching grants contribute their smallest limits. The permit reports the first matching ID in lexical order for attribution. Expiry can shorten the monotonic deadline.
4. Map `DenyReason::code()` into the existing diagnostic and MCP contracts. Use the unknown-target response for capabilities hidden by caller policy. `auth.insufficient_scope` is available for the transport's scope challenge. Use `Permit::REASON` for allowed audit decisions and record `Effect` separately from timeout or cancellation. M3 owns cancellation signalling, request IDs and terminal result handling.

The point-of-use caller binds target facts to the handle, connected peer or reviewed operation actually used. M5 owns native file resolution, DNS and connection pinning, peer verification and credential scoping. Future adapters bind verified statement, repository, executable, working directory and source identities through the same `VerifiedTarget` boundary. Runtime configuration and transport authentication stay with their owning tasks. [Adapter enforcement](security.md#adapter-access) · [Cancellation and limits](contracts.md#cancellation-deadlines-and-limits)

## REST and file grant matching

- REST origins parse as HTTP(S) scheme, canonical host and effective port. Methods are exact and uppercase in the manifest. GET, HEAD and OPTIONS map to read. POST, PUT, PATCH and DELETE map to write. Compare the path with the origin-form path of the request that the adapter will send. Templates accept literal segments and one `{name}` placeholder per segment, including `notes/{id}.txt` for files. A placeholder consumes a nonempty portion of its segment, with the same value at repeated occurrences. Percent-encoded HTTP paths decode once for matching. Escaped separators, dot segments, ambiguous encodings, queries and fragments fail path validation. The root path and trailing slash are significant.
- When `allowedPeerRanges` is omitted, the verified peer must fall outside the conservative special-use exclusions. An explicit CIDR list restricts the peers for that grant. CIDRs must name canonical subnets. An empty list denies every peer. Each request and approval applies its own check. IPv4-mapped peers use the IPv4 address. Redirects require an exact route in the operator approval and a matching requested and approved target and peer on the new hop. The adapter applies the same policy to each hop and scopes credentials to the original upstream. [IPv4 special-purpose registry](https://www.iana.org/assignments/iana-ipv4-special-registry) · [IPv6 special-purpose registry](https://www.iana.org/assignments/iana-ipv6-special-registry) · [HTTP method semantics](https://www.rfc-editor.org/rfc/rfc9110.html#section-9.2.1)
- File grant paths use slash-separated, case-sensitive portable components. An omitted `relativePathTemplate` covers safe relative paths in the verified mapped root. Platform aliases, links, special files and races at open require checks against the native handle before the adapter uses it.

## Evidence

Project allowlisted audit metadata before serialization. For opt-in payloads, give `Redactor::new` resolved secrets, their known encodings and the scenario's sensitive JSON Pointers. Use `redact_json` on a bounded JSON value and drop the payload on error. Use `redact_text` on a complete bounded diagnostic. `BufferedText` assembles bounded raw stderr chunks and emits only a redacted result. Capture overflow discards the entire capture so a split secret cannot escape at the cut. Assert against bounded results before building sanitized evidence. M7 and V2 own evidence sinks, per-connection trace limits, credential sources and retention checks. [Evidence contract](contracts.md#scenarios-results-and-evidence)

## Windows verification

The checks ran on Windows 11 Pro 25H2 x64 (build 26200.9974), using the F1 Rust and Cargo 1.99.1 toolchain and MSVC linker. In PowerShell, load the F1 tool paths and set the target directory to this checkout before invoking Cargo. [F1 setup](development.md#f1-toolchain)

```powershell
. (Join-Path $env:LOCALAPPDATA 'McpBuilderF1\env.ps1')
$env:CARGO_TARGET_DIR = Join-Path (Get-Location).Path 'target'
cargo fmt --all --check
cargo test --locked -p mcp-policy
cargo clippy --locked -p mcp-policy --all-targets -- -D warnings
cargo test --locked --workspace --exclude mcp-builder-desktop
cargo clippy --locked --workspace --exclude mcp-builder-desktop --all-targets -- -D warnings
git diff --check
```

All checks exited 0. The policy suite passed 15 tests, and the non-desktop workspace passed 17 including the two F1 contract tests. The policy tests cover the schema 1 grant round-trip, malformed and missing inputs, local and remote allow and deny decisions, capability visibility, scope and target mismatches, redirects, private-peer constraints, and narrowing limits through a recheck. Synthetic canaries cover structured headers and bodies, assertion details, upstream errors and every byte split in buffered UTF-8 stderr. The tests assert that redacted evidence contains none of those canaries.

Native desktop and cross-platform verification remain with F2. M3 and M4 own real protocol dispatch, framing, cancellation and deadline evidence. M5 owns native handle, DNS, TLS and peer evidence. M7, V2 and release QA verify evidence sinks end to end. [Verification cases](security.md#verification-cases)
