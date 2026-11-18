import { type ComponentFixture, TestBed } from "@angular/core/testing";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { afterEach, describe, expect, it, vi } from "vitest";
import catalog from "../../../../../crates/builder-project/tests/fixtures/catalog-v1.json";
import type {
  Diagnostic,
  Discovery,
  ExportConflict,
  GenerationReceipt,
  LaunchReview,
  PreviewRun,
  TraceSnapshot,
} from "./mvp-ipc";
import type { ProjectDocument, ProjectSnapshot } from "./project-model";
import { WorkbenchComponent } from "./workbench.component";

const root = String.raw`C:\projects\catalog`;
const digest = "sha256:" + "a".repeat(64);
const revisionTwo = "sha256:" + "b".repeat(64);
const registrationId = "8d1ea3a7-18c4-4e36-8a8c-6e84af79eed1";
const snapshot: ProjectSnapshot = {
  registrationId,
  rootDisplayPath: root,
  revision: digest,
  document: catalog as ProjectDocument,
};

function success<T>(data: T) {
  return { ok: true as const, data, diagnostics: [] };
}

function failure(
  code: string,
  message: string,
  details?: unknown,
  field?: string,
) {
  const error = {
    code,
    message,
    severity: "error",
    correlationId: "test",
    details,
    field,
  } satisfies Diagnostic;
  return { ok: false as const, error, diagnostics: [] };
}

type Handler = (command: string, data: Record<string, unknown>) => unknown;

async function createWorkspace(handlers: Handler): Promise<{
  fixture: ComponentFixture<WorkbenchComponent>;
  invoke: ReturnType<typeof vi.fn<Handler>>;
}> {
  const invoke = vi.fn<Handler>(handlers);
  mockIPC((command, payload) =>
    invoke(command, (payload ?? {}) as Record<string, unknown>),
  );
  const fixture = TestBed.createComponent(WorkbenchComponent);
  await fixture.whenStable();
  return { fixture, invoke };
}

function get<T extends HTMLElement>(
  fixture: ComponentFixture<WorkbenchComponent>,
  id: string,
): T {
  const control = (
    fixture.nativeElement as HTMLElement
  ).ownerDocument.getElementById(id);
  if (!control) throw new Error("Could not find #" + id);
  return control as T;
}

async function click(
  fixture: ComponentFixture<WorkbenchComponent>,
  label: string,
): Promise<void> {
  const found = Array.from(
    (fixture.nativeElement as HTMLElement).querySelectorAll("button"),
  ).find(
    (button) =>
      button.textContent?.trim() === label && !button.closest("[hidden]"),
  );
  if (!found) throw new Error("Could not find button " + label);
  found.click();
  await fixture.whenStable();
}

async function edit(
  fixture: ComponentFixture<WorkbenchComponent>,
  id: string,
  value: string,
  event: "input" | "change" = "input",
): Promise<void> {
  const control = get<
    HTMLInputElement | HTMLSelectElement | HTMLTextAreaElement
  >(fixture, id);
  control.value = value;
  control.dispatchEvent(new Event(event, { bubbles: true }));
  await fixture.whenStable();
}

function visibleText(fixture: ComponentFixture<WorkbenchComponent>): string {
  const content = (fixture.nativeElement as HTMLElement).cloneNode(
    true,
  ) as HTMLElement;
  content.querySelectorAll("[hidden]").forEach((element) => element.remove());
  return content.textContent ?? "";
}

async function openFolder(
  fixture: ComponentFixture<WorkbenchComponent>,
): Promise<void> {
  await click(fixture, "Open folder");
}

describe("desktop MVP workflow", () => {
  afterEach(() => clearMocks());

  it("creates through a native picker and validates guided fields against the same project contract", async () => {
    const empty: ProjectSnapshot = {
      ...snapshot,
      document: {
        ...snapshot.document,
        name: "New catalog",
        capabilities: [],
        bindings: [],
        policy: { grants: [] },
        secretRefs: [],
        scenarios: [],
      },
    };
    const { fixture, invoke } = await createWorkspace((command, data) => {
      switch (command) {
        case "project_list":
          return success([]);
        case "select_path":
          return success({ selectionId: "selected-folder", displayPath: root });
        case "project_create":
          return success(empty);
        case "project_validate":
          return failure(
            "project.invalid",
            "Enter an HTTPS origin.",
            undefined,
            "/bindings/0/origin",
          );
        case "project_save":
          return success({
            ...empty,
            revision: revisionTwo,
            document: (data["request"] as { document: ProjectDocument })
              .document,
          });
        default:
          throw new Error(JSON.stringify({ command, data }));
      }
    });

    await click(fixture, "New project");
    await edit(fixture, "new-project-name", "New catalog");
    await click(fixture, "Choose project folder");
    await click(fixture, "Create project");
    expect(invoke).toHaveBeenCalledWith("select_path", {
      request: { purpose: "projectDestination" },
    });
    expect(invoke).toHaveBeenCalledWith("project_create", {
      request: {
        name: "New catalog",
        destination: { selectionId: "selected-folder" },
      },
    });

    await click(fixture, "REST tool");
    await edit(fixture, "editor-field-/capabilities/0/inputSchema", "{");
    await click(fixture, "Validate");
    expect(visibleText(fixture)).toContain("Enter valid JSON.");
    expect(
      invoke.mock.calls.filter(([command]) => command === "project_validate"),
    ).toHaveLength(0);

    await edit(
      fixture,
      "editor-field-/capabilities/0/inputSchema",
      '{"type":"object"}',
    );
    await click(fixture, "Validate");
    expect(invoke).toHaveBeenCalledWith("project_validate", {
      request: {
        document: expect.objectContaining({
          capabilities: [
            expect.objectContaining({
              kind: "tool",
              inputSchema: { type: "object" },
            }),
          ],
          bindings: [expect.objectContaining({ kind: "rest", origin: "" })],
        }),
      },
    });
    expect(
      get(fixture, "editor-field-/bindings/0/origin").getAttribute(
        "aria-invalid",
      ),
    ).toBe("true");
    expect(visibleText(fixture)).toContain("Enter an HTTPS origin.");
    await click(fixture, "Go to field");
    expect(
      get(fixture, "editor-field-/bindings/0/origin").ownerDocument
        .activeElement?.id,
    ).toBe("editor-field-/bindings/0/origin");
    await edit(
      fixture,
      "editor-field-/bindings/0/origin",
      "https://catalog.example.test",
    );
    await edit(fixture, "editor-field-/bindings/0/pathTemplate", "/items");
    await click(fixture, "Request a grant from these settings");
    await click(fixture, "Add fixture scenario");
    await edit(fixture, "editor-field-/scenarios/0/name", "Read sample item");
    await edit(
      fixture,
      "editor-field-/scenarios/0/fixtures",
      '[{"kind":"rest","bindingId":"rest_tool_binding","method":"GET","path":"/items","status":200,"body":{"name":"Sample item"}}]',
    );
    await edit(
      fixture,
      "editor-field-/scenarios/0/assertions",
      '[{"kind":"equals","path":"/structuredContent/name","expected":"Sample item"}]',
    );
    await click(fixture, "Save project");
    expect(invoke).toHaveBeenCalledWith("project_save", {
      request: {
        registrationId,
        expectedRevision: digest,
        document: expect.objectContaining({
          policy: {
            grants: [
              expect.objectContaining({
                operation: "http.read",
                capabilityId: "rest_tool",
              }),
            ],
          },
          scenarios: [
            expect.objectContaining({
              version: 1,
              name: "Read sample item",
              mode: "fixture",
              fixtures: [
                expect.objectContaining({
                  kind: "rest",
                  bindingId: "rest_tool_binding",
                }),
              ],
              assertions: [
                {
                  kind: "equals",
                  path: "/structuredContent/name",
                  expected: "Sample item",
                },
              ],
            }),
          ],
        }),
      },
    });
    await click(fixture, "Playground");
    expect(
      get<HTMLSelectElement>(fixture, "fixture-scenario").selectedOptions[0]
        .textContent,
    ).toContain("Read sample item");
  });

  it("keeps a losing draft after a revision conflict and saves it as a new project", async () => {
    const { fixture, invoke } = await createWorkspace((command, data) => {
      switch (command) {
        case "project_list":
          return success([]);
        case "select_path":
          return (data["request"] as { purpose: string }).purpose ===
            "recoveryDestination"
            ? success({
                selectionId: "destination",
                displayPath: root + "-draft",
              })
            : success({ selectionId: "source", displayPath: root });
        case "project_open":
          return success(snapshot);
        case "project_save":
          return failure(
            "project.revision_conflict",
            "The project changed on disk.",
            { currentRevision: revisionTwo },
          );
        case "project_recover":
          return success({
            ...snapshot,
            registrationId: "copy",
            rootDisplayPath: root + "-draft",
            document: { ...snapshot.document, name: "Author's draft" },
          });
        default:
          throw new Error(command);
      }
    });

    await openFolder(fixture);
    await edit(fixture, "editor-field-/name", "Author's draft");
    await click(fixture, "Save project");
    expect(invoke).toHaveBeenCalledWith("project_save", {
      request: {
        registrationId,
        expectedRevision: digest,
        document: expect.objectContaining({ name: "Author's draft" }),
      },
    });
    expect(visibleText(fixture)).toContain("The project changed on disk");
    expect(get<HTMLInputElement>(fixture, "editor-field-/name").value).toBe(
      "Author's draft",
    );

    await click(fixture, "Reload saved revision");
    expect(visibleText(fixture)).toContain("Confirm project change");
    await click(fixture, "Go back");
    expect(get<HTMLInputElement>(fixture, "editor-field-/name").value).toBe(
      "Author's draft",
    );
    expect(
      invoke.mock.calls.filter(([command]) => command === "project_open"),
    ).toHaveLength(1);

    await click(fixture, "Save draft as a copy");
    expect(invoke).toHaveBeenCalledWith("project_recover", {
      request: {
        registrationId,
        destination: { selectionId: "destination" },
        source: {
          kind: "draft",
          document: expect.objectContaining({ name: "Author's draft" }),
        },
      },
    });
    expect(visibleText(fixture)).toContain("Draft saved as a new project.");
  });

  it("reviews a fixture launch before spawning, replays it, and clears transient results on stop", async () => {
    const [grant] = snapshot.document.policy.grants;
    const review: LaunchReview = {
      reviewId: "review",
      digest,
      summary: {
        digest,
        registrationId,
        projectId: snapshot.document.projectId,
        manifestRevision: digest,
        mode: { kind: "fixture", scenarioId: "known_item" },
        executable: String.raw`C:\app\mcp-builder.exe`,
        arguments: ["managed-worker"],
        workingDirectory: String.raw`C:\app`,
        environment: {},
        approvedGrants: [grant],
        approvedPromptIds: [],
        fileRoots: {},
        credentialSources: {},
        credentialTargets: [],
        limits: {},
      },
    };
    const discovery: Discovery = {
      protocolRevision: "2026-07-28",
      capabilities: { tools: true, resources: false, prompts: false },
      supportedMethods: ["tools/call"],
      tools: [{ name: "lookup_item", inputSchema: { type: "object" } }],
      resources: [],
      resourceTemplates: [],
      prompts: [],
    };
    const trace: TraceSnapshot = {
      summary: { entries: 1, droppedEntries: 2, truncated: true },
      entries: [
        {
          sequence: 4,
          offsetMs: 12,
          direction: "received",
          kind: "response",
          truncated: true,
          correlationId: "operation",
          payload: { credential: "[REDACTED]" },
        },
      ],
    };
    const run: PreviewRun = {
      result: {
        version: 1,
        runId: "run",
        correlationId: "operation",
        scenarioId: "known_item",
        target: "managedFixture",
        transport: "stdio",
        protocolRevision: "2026-07-28",
        startedAt: "2026-11-18T09:00:00Z",
        durationMs: 14,
        outcome: "passed",
        effect: "none",
        assertions: [{ index: 0, passed: true }],
        traceSummary: trace.summary,
      },
      payload: { name: "Sample item" },
    };

    const { fixture, invoke } = await createWorkspace((command, data) => {
      switch (command) {
        case "project_list":
          return success([]);
        case "select_path":
          return success({ selectionId: "source", displayPath: root });
        case "project_open":
          return success(snapshot);
        case "policy_review":
          return success(review);
        case "policy_approve":
          return success({
            approvalId: "approved",
            expiresAt: "2026-11-18T09:05:00Z",
          });
        case "playground_start":
          return success({ operationId: "start" });
        case "scenario_run":
          return success({ runId: "run" });
        case "run_poll":
          return success({
            nextCursor: 1,
            terminal: true,
            events:
              (data["request"] as { id: string }).id === "start"
                ? [
                    {
                      cursor: 1,
                      kind: "started",
                      connectionId: "managed",
                      discovery,
                    },
                  ]
                : [{ cursor: 1, kind: "completed", run }],
          });
        case "playground_trace":
          return success(trace);
        case "playground_stop":
          return success({ stopped: true });
        default:
          throw new Error(command);
      }
    });

    await openFolder(fixture);
    await click(fixture, "Playground");
    const permission = Array.from(
      (fixture.nativeElement as HTMLElement).querySelectorAll<HTMLLabelElement>(
        ".check-label",
      ),
    )
      .find((label) => label.textContent?.includes("catalog_read"))
      ?.querySelector("input");
    expect(permission).toBeDefined();
    permission!.click();
    await fixture.whenStable();
    await click(fixture, "Review launch");
    expect(invoke).toHaveBeenCalledWith("policy_review", {
      request: {
        subject: { kind: "launch", registrationId, expectedRevision: digest },
        mode: { kind: "fixture", scenarioId: "known_item" },
        access: { approvedGrantIds: ["catalog_read"], approvedPromptIds: [] },
        protocol: "auto",
        capturePayloads: false,
      },
    });
    expect(visibleText(fixture)).toContain(review.summary.executable);
    await click(fixture, "Cancel launch review");
    expect(
      invoke.mock.calls.filter(
        ([command]) =>
          command === "policy_approve" || command === "playground_start",
      ),
    ).toEqual([]);

    await click(fixture, "Review launch");
    await click(fixture, "Approve and start");
    expect(invoke).toHaveBeenCalledWith("policy_approve", {
      request: { reviewId: "review", digest },
    });
    expect(invoke).toHaveBeenCalledWith("playground_start", {
      request: {
        subject: { kind: "launch", registrationId, expectedRevision: digest },
        approval: { approvalId: "approved" },
      },
    });
    expect(visibleText(fixture)).toContain("Connected over stdio");
    expect(visibleText(fixture)).toContain("lookup_item");
    await click(fixture, "Run saved fixture");
    expect(invoke).toHaveBeenCalledWith("scenario_run", {
      request: {
        connectionId: "managed",
        registrationId,
        expectedRevision: digest,
        scenarioId: "known_item",
      },
    });
    expect(visibleText(fixture)).toContain("Passed in 14 ms.");
    expect(visibleText(fixture)).toContain("trace truncated");
    expect(visibleText(fixture)).toContain("[REDACTED]");

    await click(fixture, "Stop server");
    expect(invoke).toHaveBeenCalledWith("playground_stop", {
      request: { connectionId: "managed" },
    });
    expect(visibleText(fixture)).not.toContain("Sample item");
    expect(visibleText(fixture)).not.toContain("[REDACTED]");
  });

  it("writes a reviewed export only after every conflict has an explicit decision", async () => {
    const conflict: ExportConflict = {
      path: "Cargo.toml",
      previousDigest: digest,
      observedDigest: revisionTwo,
      replacementDigest: digest,
      canKeep: false,
    };
    const output: GenerationReceipt = {
      destinationDisplayPath: String.raw`C:\exports\catalog`,
      generation: {
        version: 1,
        projectId: snapshot.document.projectId,
        projectSchemaVersion: 1,
        snapshotDigest: digest,
        generatorVersion: "0.1.0",
        runtimeVersion: "0.1.0",
        templateVersion: "0.1.0",
        protocolVersions: ["2026-07-28"],
        cargoLockDigest: digest,
        files: [{ path: "Cargo.toml", owner: "generator", digest }],
      },
    };
    const { fixture, invoke } = await createWorkspace((command, data) => {
      switch (command) {
        case "project_list":
          return success([]);
        case "select_path": {
          const purpose = (data["request"] as { purpose: string }).purpose;
          return success({ selectionId: purpose, displayPath: root });
        }
        case "project_open":
          return success(snapshot);
        case "generation_plan":
          return success({
            ...output,
            planId: "plan",
            planDigest: digest,
            generation: null,
            conflicts: [conflict],
          });
        case "generation_write":
          return success(output);
        default:
          throw new Error(command);
      }
    });
    await openFolder(fixture);
    await click(fixture, "Export");
    await edit(fixture, "export-folder-name", "catalog");
    await click(fixture, "Choose parent folder");
    await click(fixture, "Choose previous export (optional)");
    await click(fixture, "Review export");
    expect(invoke).toHaveBeenCalledWith("generation_plan", {
      request: {
        registrationId,
        expectedRevision: digest,
        destination: { selectionId: "exportParent" },
        folderName: "catalog",
        fromExport: { selectionId: "previousExport" },
      },
    });
    await click(fixture, "Export Rust source");
    expect(
      invoke.mock.calls.filter(([command]) => command === "generation_write"),
    ).toEqual([]);
    expect(
      get<HTMLSelectElement>(fixture, "export-conflict-0").querySelector(
        "[value='keep']",
      ),
    ).toBeNull();

    await edit(fixture, "export-conflict-0", "regenerate", "change");
    await click(fixture, "Export Rust source");
    expect(invoke).toHaveBeenCalledWith("generation_write", {
      request: {
        planId: "plan",
        planDigest: digest,
        resolutions: [
          {
            path: "Cargo.toml",
            observedDigest: revisionTwo,
            replacementDigest: digest,
            decision: "regenerate",
          },
        ],
      },
    });
    expect(visibleText(fixture)).toContain("Export complete");
  });
});
