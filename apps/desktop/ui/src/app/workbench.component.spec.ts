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

async function toggleCheck(
  fixture: ComponentFixture<WorkbenchComponent>,
  label: string,
): Promise<void> {
  const control = Array.from(
    (fixture.nativeElement as HTMLElement).querySelectorAll<HTMLLabelElement>(
      ".check-label",
    ),
  )
    .find((item) => item.textContent?.includes(label))
    ?.querySelector("input");
  if (!control) throw new Error("Could not find checkbox " + label);
  control.click();
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

  it("configures file paths and prompt arguments and saves a fixture that exercises the prompt", async () => {
    const empty: ProjectSnapshot = {
      ...snapshot,
      document: {
        ...snapshot.document,
        capabilities: [],
        bindings: [],
        policy: { grants: [] },
        secretRefs: [],
        scenarios: [],
      },
    };
    const { fixture, invoke } = await createWorkspace(
      (command, { request }) => {
        switch (command) {
          case "project_list":
            return success([]);
          case "select_path":
            return success({ selectionId: "source", displayPath: root });
          case "project_open":
            return success(empty);
          case "project_validate":
            return success({ valid: true });
          case "project_save":
            return success({
              ...empty,
              revision: revisionTwo,
              document: (request as { document: ProjectDocument }).document,
            });
          default:
            throw new Error(command);
        }
      },
    );

    await openFolder(fixture);
    await click(fixture, "File resource");
    await edit(fixture, "editor-field-/capabilities/0/name", "Catalog guide");
    await edit(
      fixture,
      "editor-field-/capabilities/0/uri",
      "catalog://docs/guide",
    );
    await edit(fixture, "editor-field-/bindings/0/rootRef", "catalog_docs");
    await edit(
      fixture,
      "editor-field-/bindings/0/relativePathTemplate",
      "manual/guide.txt",
    );
    await edit(fixture, "editor-field-/capabilities/0/timeoutMs", "2.5");
    await click(fixture, "Validate");
    expect(visibleText(fixture)).toContain("Enter a whole number");
    expect(
      invoke.mock.calls.some(([command]) => command === "project_validate"),
    ).toBe(false);
    await edit(fixture, "editor-field-/capabilities/0/timeoutMs", "2000");
    await click(fixture, "Request a grant from these settings");
    await click(fixture, "Request a grant from these settings");
    await click(fixture, "Remove grant grant_2");

    await click(fixture, "Resource template");
    await edit(fixture, "editor-field-/capabilities/1/name", "Catalog note");
    await edit(
      fixture,
      "editor-field-/capabilities/1/uriTemplate",
      "catalog://docs/items/{id}",
    );
    await edit(
      fixture,
      "editor-field-/capabilities/1/variables",
      '{"id":{"type":"string","pattern":"^[0-9]+$"}}',
    );
    await edit(fixture, "editor-field-/bindings/1/rootRef", "catalog_docs");
    await edit(
      fixture,
      "editor-field-/bindings/1/relativePathTemplate",
      "notes/{id}.txt",
    );
    await click(fixture, "Request a grant from these settings");
    await edit(
      fixture,
      "editor-field-/policy/grants/1/target/relativePathTemplate",
      "notes/42.txt",
    );
    await edit(
      fixture,
      "editor-field-/policy/grants/1/limits/timeoutMs",
      "1200",
    );

    await click(fixture, "Prompt");
    await edit(fixture, "editor-field-/capabilities/2/name", "review_item");
    await click(fixture, "Add prompt argument");
    await edit(
      fixture,
      "editor-field-/capabilities/2/arguments/0/name",
      "draft_placeholder",
    );
    await click(fixture, "Add prompt argument");
    await edit(fixture, "editor-field-/capabilities/2/arguments/1/name", "id");
    await click(fixture, "Remove argument 1");
    const required = get<HTMLInputElement>(
      fixture,
      "editor-field-/capabilities/2/arguments/0/required",
    );
    required.click();
    await fixture.whenStable();
    await edit(
      fixture,
      "editor-field-/capabilities/2/messages/0/textTemplate",
      "Review the catalog.",
    );
    await click(fixture, "Add prompt message");
    await edit(
      fixture,
      "editor-field-/capabilities/2/messages/1/role",
      "assistant",
      "change",
    );
    await edit(
      fixture,
      "editor-field-/capabilities/2/messages/1/textTemplate",
      "Review item {id}.",
    );

    await click(fixture, "Add fixture scenario");
    await edit(fixture, "editor-field-/scenarios/0/name", "Render review");
    await edit(fixture, "scenario-method", "prompts/get", "change");
    await edit(
      fixture,
      "editor-field-/scenarios/0/call/arguments",
      '{"id":"42"}',
    );
    await edit(
      fixture,
      "editor-field-/scenarios/0/assertions",
      '[{"kind":"schema","path":"/messages","schema":{"type":"array"}}]',
    );
    await click(fixture, "Validate");
    expect(visibleText(fixture)).toContain("The draft is valid.");
    await click(fixture, "Save project");

    expect(invoke).toHaveBeenCalledWith("project_save", {
      request: {
        registrationId,
        expectedRevision: digest,
        document: expect.objectContaining({
          capabilities: [
            expect.objectContaining({
              kind: "resource",
              uri: "catalog://docs/guide",
              timeoutMs: 2000,
            }),
            expect.objectContaining({
              kind: "resourceTemplate",
              uriTemplate: "catalog://docs/items/{id}",
              variables: { id: { type: "string", pattern: "^[0-9]+$" } },
            }),
            expect.objectContaining({
              kind: "prompt",
              name: "review_item",
              arguments: [{ name: "id", required: false }],
              messages: [
                { role: "user", textTemplate: "Review the catalog." },
                { role: "assistant", textTemplate: "Review item {id}." },
              ],
            }),
          ],
          bindings: [
            expect.objectContaining({
              kind: "fileRead",
              rootRef: "catalog_docs",
              relativePathTemplate: "manual/guide.txt",
            }),
            expect.objectContaining({
              kind: "fileRead",
              rootRef: "catalog_docs",
              relativePathTemplate: "notes/{id}.txt",
            }),
          ],
          policy: {
            grants: [
              expect.objectContaining({
                capabilityId: "file_resource",
                operation: "file.read",
              }),
              expect.objectContaining({
                capabilityId: "file_template",
                operation: "file.read",
                target: {
                  kind: "fileRead",
                  rootRef: "catalog_docs",
                  relativePathTemplate: "notes/42.txt",
                },
                limits: { timeoutMs: 1200 },
              }),
            ],
          },
          scenarios: [
            expect.objectContaining({
              name: "Render review",
              mode: "fixture",
              call: {
                method: "prompts/get",
                name: "review_item",
                arguments: { id: "42" },
              },
            }),
          ],
        }),
      },
    });
    expect(visibleText(fixture)).toContain("Project saved.");
  });

  it("repicks the same damaged project before restoring its last valid revision", async () => {
    const messages = failure(
      "project.missing",
      "The project manifest is missing.",
    );
    const candidate = {
      kind: "lastValid",
      path: ".mcp-project.last-valid.json",
      revision: digest,
    };
    let pickerNumber = 0;
    const { fixture, invoke } = await createWorkspace((command) => {
      switch (command) {
        case "project_list":
          return success([]);
        case "select_path": {
          pickerNumber++;
          return success({
            selectionId: "selection-" + pickerNumber,
            displayPath: pickerNumber === 2 ? root + "-other" : root,
          });
        }
        case "project_open":
          return {
            ...messages,
            diagnostics: [
              {
                ...messages.error,
                code: "project.recovery_available",
                severity: "warning",
                message: "A valid local recovery file is available.",
                details: {
                  currentRevision: null,
                  candidates: [
                    candidate,
                    {
                      kind: "interruptedWrite",
                      path: ".mcp-project-stage-1",
                      revision: revisionTwo,
                    },
                  ],
                },
              },
            ],
          };
        case "project_recover":
          return success(snapshot);
        default:
          throw new Error(command);
      }
    });
    await openFolder(fixture);
    expect(visibleText(fixture)).toContain("The project manifest is missing.");
    expect(visibleText(fixture)).toContain("Interrupted write");
    await click(fixture, "Restore this revision");
    expect(
      invoke.mock.calls.some(([command]) => command === "project_recover"),
    ).toBe(false);
    await click(fixture, "Continue");
    expect(visibleText(fixture)).toContain(
      "Select the project folder that reported this recovery file.",
    );
    expect(
      invoke.mock.calls.some(([command]) => command === "project_recover"),
    ).toBe(false);

    await click(fixture, "Restore this revision");
    await click(fixture, "Continue");
    expect(invoke).toHaveBeenCalledWith("project_recover", {
      request: {
        root: { selectionId: "selection-3" },
        expectedRevision: null,
        source: { kind: "backup", revision: digest },
      },
    });
    expect(visibleText(fixture)).toContain(
      "Project restored from the selected revision.",
    );
    expect(visibleText(fixture)).toContain("Catalog sample");
  });

  it("reviews explicit live sources, clears the session value, and accepts another call after cancellation", async () => {
    const liveRoot = root + "\\docs";
    const canary = "one-launch-component-secret";
    const environmentName = "MCP_PILOT_TOKEN";
    const liveProject: ProjectSnapshot = {
      ...snapshot,
      document: {
        ...snapshot.document,
        secretRefs: [
          ...snapshot.document.secretRefs,
          { id: "catalog_session", purpose: "Authorize this session" },
        ],
        bindings: snapshot.document.bindings.map((binding) =>
          binding.kind === "rest"
            ? {
                ...binding,
                headersFromSecrets: {
                  ...binding.headersFromSecrets,
                  "X-Catalog-Session": "catalog_session",
                },
              }
            : binding,
        ),
      },
    };
    const discovery: Discovery = {
      protocolRevision: "2025-11-25",
      supportedMethods: ["tools/call", "resources/read", "prompts/get"],
      capabilities: { tools: true, resources: true, prompts: true },
      tools: [{ name: "lookup_item", inputSchema: { type: "object" } }],
      resources: [],
      resourceTemplates: [
        { name: "Catalog note", uriTemplate: "catalog://docs/items/{id}" },
      ],
      prompts: [
        { name: "review_item", arguments: [{ name: "id", required: true }] },
      ],
    };
    const review: LaunchReview = {
      reviewId: "live-review",
      digest,
      summary: {
        digest,
        registrationId,
        projectId: snapshot.document.projectId,
        manifestRevision: digest,
        mode: { kind: "live" },
        executable: root + "\\mcp-builder.exe",
        arguments: ["managed-worker"],
        workingDirectory: root,
        environment: {},
        fileRoots: { catalog_docs: liveRoot },
        approvedGrants: liveProject.document.policy.grants,
        approvedPromptIds: ["review_item"],
        credentialSources: {
          catalog_auth: { kind: "environment", name: environmentName },
          catalog_session: { kind: "session" },
        },
        credentialTargets: [
          {
            capabilityId: "lookup_item",
            bindingId: "catalog_lookup",
            origin: "https://catalog.example.test",
            references: ["catalog_auth", "catalog_session"],
          },
        ],
        limits: {},
      },
    };
    const trace: TraceSnapshot = {
      summary: { entries: 0, droppedEntries: 0, truncated: false },
      entries: [],
    };
    const run = (
      runId: string,
      outcome: "cancelled" | "passed",
    ): PreviewRun => ({
      result: {
        version: 1,
        runId,
        correlationId: runId,
        target: "managedLive",
        transport: "stdio",
        protocolRevision: "2025-11-25",
        startedAt: "2026-11-18T09:00:00Z",
        durationMs: 12,
        outcome,
        effect: "none",
        assertions: [],
        traceSummary: trace.summary,
      },
      ...(outcome === "passed"
        ? {
            payload: {
              messages: [{ role: "user", content: "Review catalog item 42." }],
            },
          }
        : {}),
    });

    let rootSelections = 0;
    let cancelled = false;
    let password: HTMLInputElement | undefined;
    let passwordDuringReview: string | undefined;
    let stopAttempts = 0;
    const { fixture, invoke } = await createWorkspace(
      (command, { request }) => {
        switch (command) {
          case "project_list":
            return success([]);
          case "select_path":
            return (request as { purpose: string }).purpose === "fileRoot"
              ? success({
                  selectionId: "file-root-" + ++rootSelections,
                  displayPath: liveRoot,
                })
              : success({ selectionId: "project-root", displayPath: root });
          case "project_open":
            return success(liveProject);
          case "policy_review":
            passwordDuringReview = password?.value;
            return success(review);
          case "policy_approve":
            return success({
              approvalId: "approved-live",
              expiresAt: "2026-11-18T09:05:00Z",
            });
          case "playground_start":
            return success({ operationId: "start-live" });
          case "playground_call":
            return success({
              runId:
                (request as { call: { method: string } }).call.method ===
                "resources/read"
                  ? "file-run"
                  : "prompt-run",
            });
          case "run_poll": {
            const { id } = request as { id: string };
            if (id === "file-run" && !cancelled)
              return success({ events: [], nextCursor: 0, terminal: false });
            const event =
              id === "start-live"
                ? { kind: "started", connectionId: "live", discovery }
                : {
                    kind: "completed",
                    run: run(id, id === "file-run" ? "cancelled" : "passed"),
                  };
            return success({
              events: [{ cursor: 1, ...event }],
              nextCursor: 1,
              terminal: true,
            });
          }
          case "run_cancel":
            cancelled = (request as { id: string }).id === "file-run";
            return success({ cancellationRequested: cancelled });
          case "playground_trace":
            return success(trace);
          case "playground_stop":
            if (++stopAttempts === 1) throw new Error(canary);
            return failure(
              "playground.stop_failed",
              "The preview process did not stop within its shutdown budget.",
            );
          default:
            throw new Error(command);
        }
      },
    );

    await openFolder(fixture);
    await click(fixture, "Playground");
    await edit(fixture, "preview-mode", "live", "change");
    await edit(fixture, "preview-protocol", "2025-11-25", "change");
    await toggleCheck(fixture, "catalog_read");
    await toggleCheck(fixture, "note_read");
    await toggleCheck(fixture, "review_item");
    await toggleCheck(fixture, "Include redacted message contents");
    await click(fixture, "Choose folder for catalog_docs");
    expect(visibleText(fixture)).toContain(liveRoot);
    await click(fixture, "Clear catalog_docs");
    expect(visibleText(fixture)).not.toContain(liveRoot);
    await click(fixture, "Choose folder for catalog_docs");
    await edit(fixture, "secret-source-catalog_auth", "environment", "change");
    await edit(fixture, "secret-catalog_auth", environmentName);
    await edit(fixture, "secret-source-catalog_session", "session", "change");
    await edit(fixture, "secret-catalog_session", canary);
    password = get<HTMLInputElement>(fixture, "secret-catalog_session");
    expect(password.type).toBe("password");

    await click(fixture, "Review launch");
    expect(passwordDuringReview).toBe("");
    expect(password.value).toBe("");
    expect(invoke).toHaveBeenCalledWith("policy_review", {
      request: {
        subject: { kind: "launch", registrationId, expectedRevision: digest },
        mode: { kind: "live" },
        access: {
          approvedGrantIds: ["catalog_read", "note_read"],
          approvedPromptIds: ["review_item"],
          fileRoots: { catalog_docs: { selectionId: "file-root-2" } },
          credentials: {
            catalog_auth: { kind: "environment", name: environmentName },
            catalog_session: { kind: "session", value: canary },
          },
        },
        protocol: "2025-11-25",
        capturePayloads: true,
      },
    });
    expect(visibleText(fixture)).toContain(environmentName);
    expect(visibleText(fixture)).toContain("Session value");
    expect(visibleText(fixture)).toContain(liveRoot);
    expect(visibleText(fixture)).not.toContain(canary);
    await click(fixture, "Approve and start");

    await click(fixture, "Use template Catalog note");
    await edit(fixture, "playground-target", "catalog://docs/items/42");
    await edit(fixture, "playground-timeout", "30001");
    await click(fixture, "Run live request");
    expect(visibleText(fixture)).toContain(
      "Enter a timeout from 1 to 30,000 ms.",
    );
    expect(
      invoke.mock.calls.some(([command]) => command === "playground_call"),
    ).toBe(false);
    await edit(fixture, "playground-timeout", "1000");
    await click(fixture, "Run live request");
    expect(invoke).toHaveBeenCalledWith("playground_call", {
      request: {
        connectionId: "live",
        timeoutMs: 1000,
        call: { method: "resources/read", uri: "catalog://docs/items/42" },
      },
    });
    await click(fixture, "Cancel call");
    expect(invoke).toHaveBeenCalledWith("run_cancel", {
      request: { id: "file-run" },
    });
    await vi.waitFor(async () => {
      await fixture.whenStable();
      expect(visibleText(fixture)).toContain("Cancelled in 12 ms.");
    });

    await edit(fixture, "playground-method", "prompts/get", "change");
    await edit(fixture, "playground-target", "review_item");
    await edit(fixture, "playground-arguments", '{"id":42}');
    await click(fixture, "Run live request");
    expect(visibleText(fixture)).toContain("JSON object with string values");
    await edit(fixture, "playground-arguments", '{"id":"42"}');
    await click(fixture, "Run live request");
    expect(invoke).toHaveBeenCalledWith("playground_call", {
      request: {
        connectionId: "live",
        timeoutMs: 1000,
        call: {
          method: "prompts/get",
          name: "review_item",
          arguments: { id: "42" },
        },
      },
    });
    expect(visibleText(fixture)).toContain("Review catalog item 42.");

    await click(fixture, "Stop server");
    expect(visibleText(fixture)).toContain("Try Stop server again.");
    expect(visibleText(fixture)).not.toContain(canary);
    await click(fixture, "Stop server");
    expect(visibleText(fixture)).toContain(
      "The connection closed with a shutdown error.",
    );
    expect(visibleText(fixture)).not.toContain("Review catalog item 42.");
    expect(get<HTMLInputElement>(fixture, "secret-catalog_session").value).toBe(
      "",
    );
    expect(visibleText(fixture)).not.toContain(liveRoot);
  });
});
