# MCP Server Builder commercial model

Phase 0 draft · 12 November 2026 · [Product requirements](requirements.md)

## Competitor workflows

The comparison describes workflows documented by the product teams as of 12 November 2026. Hands-on timing, usability, and willingness to pay still need pilot evidence.

| Alternative | Documented workflow | Comparison task |
| --- | --- | --- |
| MCP Inspector | The reference tester takes a launch command or server URL. Its web, CLI, and terminal clients connect to servers. The web UI exercises capabilities and exposes the JSON-RPC transcript. [MCP Inspector overview](https://modelcontextprotocol.io/docs/2026-07-28/tools/inspector), [Inspector web client](https://modelcontextprotocol.io/docs/2026-07-28/tools/inspector/web) | Start with the same server and compare discovery, call inspection, and the steps needed to save and replay an assertion. |
| FastMCP | The Python framework exposes application functions as MCP capabilities with generated schemas. Its guides cover component authorization, Pytest fixtures and assertions, and HTTP deployment. The main documentation can describe features ahead of stable releases. [Framework overview](https://gofastmcp.com/docs/getting-started/welcome), [Authorization](https://gofastmcp.com/docs/servers/authorization), [Testing](https://gofastmcp.com/docs/servers/testing), [HTTP deployment](https://gofastmcp.com/docs/deployment/http) | Give Python developers a small API and file integration. Measure the effort to implement, review access, save repeatable tests, and hand the source to a second maintainer. Pin the version tested. |
| Speakeasy standalone MCP | Its CLI converts an OpenAPI document into a standalone TypeScript server. It supports operation scoping, optional Cloudflare deployment inputs, and an opt-in model-driven `eval` command. [Generation overview](https://www.speakeasy.com/docs/standalone-mcp/overview), [Generation guide](https://www.speakeasy.com/docs/standalone-mcp/build-server), [Eval guide](https://www.speakeasy.com/docs/standalone-mcp/eval-command) | Give both workflows the same OpenAPI-described endpoint. Compare setup, the operation exposed, and whether another developer can build and run the delivered source. Measure the mixed-integration task independently. |
| Claude Code `mcp-server-dev` | The official plugin asks about the use case and scaffolds a local stdio or remote HTTP server. It also routes to skills for MCP Apps and MCP Bundle packaging. [Claude Code MCP guide](https://code.claude.com/docs/en/mcp#find-and-build-mcp-servers), [Official plugin README](https://raw.githubusercontent.com/anthropics/claude-plugins-official/main/plugins/mcp-server-dev/README.md) | Give participants the same short integration brief. Compare the first working invocation, what access a reviewer can identify, and the work needed to reproduce the result from the delivered source and fixtures. |
| n8n MCP Server Trigger | The visual workflow connects tool nodes to an MCP endpoint over Streamable HTTP or SSE, with test and production URLs and authentication options. Workflows export as JSON and run on an n8n instance, which can be self-hosted. [MCP Server Trigger](https://docs.n8n.io/integrations/builtin/core-nodes/n8n-nodes-langchain.mcptrigger), [Workflow export](https://docs.n8n.io/build/manage-workflows/export-and-import), [Self-hosting guide](https://docs.n8n.io/deploy/host-n8n) | Give teams familiar with visual automation the same internal endpoint task. Compare time to a test call, review of allowed operations, and what a second operator needs to deploy the exported artifact. |

## Positioning hypotheses

- Developers with an API and a local data source will value configuring both in one project, reviewing the access enforced in preview and export, and reproducing a failure from a saved scenario. The pilot should measure time and mistakes at each step.
- Platform teams will value the handoff of editable Rust, versioned policies, and reproducible tests to a service owner who can operate on their own infrastructure. Record when Rust or maintaining the exported code creates adoption friction.
- Existing MCP clients and CI are the likely distribution path inside engineering teams. The pilot should observe whether a developer can hand a colleague a working client configuration and whether the colleague can verify it with their usual tools.

Use the reference project for the first pilot. Recruit eight hands-on participants across AI application development and automation and four platform reviewers or operators. Time from a clean setup to the first successful call and record installation, authoring, review, and execution separately. Give matched tasks to alternatives where their documented workflows apply, rotate task order, and record versions, prerequisites, assistance, failures, and workflow limits. Use the same upstream data and expected results. [Reference project](requirements.md#reference-project)

Initial hypotheses for a pilot decision are that at least six of eight builders make an unassisted first call, the median time from opening the installed builder to that call is at most 15 minutes, and at least six of eight can explain a denial and replay a failing fixture. At least three of four operators should independently build the export, make a call, and identify the host and file scope they would approve. Record time to verified deployment with OAuth for the v1 pilot. Report counts and observed blockers alongside timings.

## Packaging and pricing hypotheses

These are candidate offers and USD price points for research. The PRD defines the v1 feature and acceptance scope. [Commercial v1 requirements](requirements.md#commercial-v1)

| Offer | Proposed value | Price to test |
| --- | --- | --- |
| Developer | Local authoring, fixture replay, core CLI workflows, source export, and built-in security and recovery. | Free. Test whether developers adopt it for a second project after using the sample. |
| Professional | Advanced automation beyond core CLI replay, such as scenario scheduling, or additional maintained template packs. | Compare $19 and $39 per developer per month against the same feature bundle. |
| Team | Shared policy distribution and centralized audit retention after those features have a validated workflow. | Test per-author pricing against a $120 monthly team bundle for up to five authors. |
| Enterprise | Procurement and support for self-hosted rollout, including response commitments and assisted upgrades. | Test annual contract interest and budget range after identifying the buyer and support needs. |

Use the workflow pilot to learn which capabilities create repeat use, then show consistent packages at varied price points in separate buyer interviews. Record the current alternative, the paid capability the buyer would use, the purchasing owner, and a concrete budget or procurement step. Treat three independent, non-binding paid-pilot commitments at a tested price as an initial signal to refine the offer. Revisit the package when buyers value only core capabilities or require features outside the planned release. Payment processing and account changes require separate authorization.

## Licensing and continuity requirements

- Keep licensing in the builder. Limit entitlement decisions to optional paid builder features. Local project access, editing, validation, testing, recovery, source export, and security enforcement remain available when an entitlement cannot be verified. Keep assets needed to work with existing projects accessible.
- Export source and runtime dependencies that build and run without the builder or a licensing endpoint. Publish the terms for editing and distributing exports alongside applicable notices and version information. Product and platform need to verify the builder license, generated-code terms, and redistribution terms for the bundled runtime and dependencies before distribution.
- Exercise missing, expired, and unreachable entitlement states using the same saved project. Confirm the project still opens, the fixture still runs, denied actions still fail, and a previously exported server builds and runs with the licensing service unavailable. [Continuity acceptance check](requirements.md#commercial-v1)

## Decisions for D5

- Set the pilot cohort, reference fixtures, comparison versions, and success thresholds. Use the results to prioritize workflow improvements and decide whether to test a Professional bundle.
- Assign ownership of entitlement continuity and export licensing. Confirm which runtime and templates can be redistributed and how customers receive security fixes for previously generated code.
- Record free and paid packaging as proposals in the backlog. Central policy, hosted audit retention, and payment integration need their own product decisions before scheduling.
