import { JsonPipe, KeyValuePipe } from "@angular/common";
import {
  ChangeDetectionStrategy,
  Component,
  DestroyRef,
  computed,
  inject,
  input,
  output,
  signal,
} from "@angular/core";
import { DiagnosticsComponent } from "./diagnostics.component";
import {
  desktop,
  diagnosticsFrom,
  handle,
  outcomeNames,
  type Diagnostic,
  type Discovery,
  type LaunchReview,
  type LaunchSubject,
  type PathSelection,
  type PreviewAccess,
  type PreviewMode,
  type PreviewRun,
  type ProtocolPreference,
  type RunEvent,
  type TraceSnapshot,
} from "./mvp-ipc";
import type {
  Invocation,
  JsonObject,
  ProjectSnapshot,
  Scenario,
} from "./project-model";

type Phase =
  "configure" | "preparing" | "review" | "starting" | "connected" | "stopping";
type Source = "none" | "environment" | "session";
type Connection = { id: string; mode: PreviewMode; discovery: Discovery };

function invocation(
  method: Invocation["method"],
  target: string,
  input: string,
): Invocation | null {
  if (!target.trim()) return null;
  if (method === "resources/read") return { method, uri: target.trim() };
  let parameters: unknown;
  try {
    parameters = JSON.parse(input);
  } catch {
    return null;
  }
  if (
    typeof parameters !== "object" ||
    parameters === null ||
    Array.isArray(parameters)
  )
    return null;
  if (method === "prompts/get") {
    if (!Object.values(parameters).every((value) => typeof value === "string"))
      return null;
    return {
      method,
      name: target.trim(),
      arguments: parameters as Record<string, string>,
    };
  }
  return { method, name: target.trim(), arguments: parameters as JsonObject };
}

@Component({
  selector: "app-playground",
  imports: [DiagnosticsComponent, JsonPipe, KeyValuePipe],
  templateUrl: "./playground.component.html",
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class PlaygroundComponent {
  readonly snapshot = input.required<ProjectSnapshot>();
  readonly blocked = input.required<boolean>();
  readonly activeChange = output<boolean>();

  private destroyed = false;
  protected readonly phase = signal<Phase>("configure");
  protected readonly mode = signal<"fixture" | "live">("fixture");
  protected readonly fixtureId = signal("");
  protected readonly protocol = signal<ProtocolPreference>("auto");
  protected readonly capturePayloads = signal(false);
  protected readonly grantIds = signal<ReadonlySet<string>>(new Set());
  protected readonly promptIds = signal<ReadonlySet<string>>(new Set());
  protected readonly roots = signal<ReadonlyMap<string, PathSelection>>(
    new Map(),
  );
  protected readonly credentialSources = signal<ReadonlyMap<string, Source>>(
    new Map(),
  );
  protected readonly selectionBusy = signal(false);
  protected readonly diagnostics = signal<Diagnostic[]>([]);
  protected readonly status = signal("");
  protected readonly review = signal<{
    subject: LaunchSubject;
    mode: PreviewMode;
    details: LaunchReview;
  } | null>(null);
  protected readonly connection = signal<Connection | null>(null);
  protected readonly operationId = signal<string | null>(null);
  protected readonly runId = signal<string | null>(null);
  protected readonly runBusy = signal(false);
  protected readonly traceBusy = signal(false);
  protected readonly trace = signal<TraceSnapshot | null>(null);
  protected readonly result = signal<PreviewRun | null>(null);
  protected readonly timeout = signal("");
  protected readonly method = signal<Invocation["method"]>("tools/call");
  protected readonly target = signal("");
  protected readonly argumentsText = signal("{}");
  protected readonly outcomeNames = outcomeNames;
  protected readonly fixtureScenarios = computed(() =>
    this.snapshot().document.scenarios.filter(
      (scenario) => scenario.mode === "fixture",
    ),
  );
  protected readonly selectedFixture = computed<Scenario | undefined>(
    () =>
      this.fixtureScenarios().find(
        (scenario) => scenario.id === this.fixtureId(),
      ) ?? this.fixtureScenarios()[0],
  );
  protected readonly prompts = computed(() =>
    this.snapshot().document.capabilities.filter(
      (item) => item.kind === "prompt",
    ),
  );
  protected readonly rootReferences = computed(() => [
    ...new Set(
      this.snapshot()
        .document.bindings.filter((item) => item.kind === "fileRead")
        .map((item) => item.rootRef),
    ),
  ]);
  protected readonly canReview = computed(
    () =>
      this.phase() === "configure" &&
      !this.blocked() &&
      !this.selectionBusy() &&
      (this.mode() === "live" || this.selectedFixture() !== undefined),
  );

  constructor() {
    inject(DestroyRef).onDestroy(() => {
      this.destroyed = true;
      const active = this.connection();
      const startup = this.operationId();
      const run = this.runId();
      if (run) void desktop("run_cancel", { id: run });
      if (startup) void desktop("run_cancel", { id: startup });
      if (active) void desktop("playground_stop", { connectionId: active.id });
    });
  }

  protected setMode(event: Event): void {
    this.mode.set(this.controlValue(event) === "live" ? "live" : "fixture");
    this.status.set("");
    this.diagnostics.set([]);
  }

  protected setFixture(event: Event): void {
    this.fixtureId.set(this.controlValue(event));
  }

  protected setProtocol(event: Event): void {
    const value = this.controlValue(event);
    if (value === "auto" || value === "2026-07-28" || value === "2025-11-25")
      this.protocol.set(value);
  }

  protected setCapture(event: Event): void {
    this.capturePayloads.set((event.target as HTMLInputElement).checked);
  }

  protected togglePermission(
    kind: "grant" | "prompt",
    id: string,
    event: Event,
  ): void {
    const set = kind === "grant" ? this.grantIds : this.promptIds;
    const selected = new Set(set());
    if ((event.target as HTMLInputElement).checked) selected.add(id);
    else selected.delete(id);
    set.set(selected);
  }

  protected setCredential(ref: string, event: Event): void {
    const source = this.controlValue(event);
    const selected = new Map(this.credentialSources());
    selected.set(
      ref,
      source === "environment" || source === "session" ? source : "none",
    );
    this.credentialSources.set(selected);
  }

  protected async chooseRoot(ref: string): Promise<void> {
    if (this.phase() !== "configure" || this.selectionBusy()) return;
    this.selectionBusy.set(true);
    try {
      const reply = await desktop("select_path", { purpose: "fileRoot" });
      this.diagnostics.set(diagnosticsFrom(reply));
      if (reply.ok && reply.data)
        this.roots.set(new Map(this.roots()).set(ref, reply.data));
    } finally {
      this.selectionBusy.set(false);
    }
  }

  protected clearRoot(ref: string): void {
    const next = new Map(this.roots());
    next.delete(ref);
    this.roots.set(next);
  }

  protected async prepare(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (!this.canReview()) return;
    const fixture = this.selectedFixture();
    const selectedMode: PreviewMode =
      this.mode() === "fixture" && fixture
        ? { kind: "fixture", scenarioId: fixture.id }
        : { kind: "live" };
    const { registrationId, revision, document } = this.snapshot();
    const subject: LaunchSubject = {
      kind: "launch",
      registrationId,
      expectedRevision: revision,
    };
    const access: PreviewAccess = {
      approvedGrantIds: document.policy.grants
        .filter((grant) => this.grantIds().has(grant.id))
        .map((grant) => grant.id),
      approvedPromptIds: this.prompts()
        .filter((prompt) => this.promptIds().has(prompt.id))
        .map((prompt) => prompt.id),
    };
    if (selectedMode.kind === "live") {
      access.fileRoots = Object.fromEntries(
        this.rootReferences().flatMap((ref) => {
          const selection = this.roots().get(ref);
          return selection ? [[ref, handle(selection)]] : [];
        }),
      );
      this.roots.set(new Map());
      const form = event.currentTarget as HTMLFormElement;
      const credentials: NonNullable<PreviewAccess["credentials"]> =
        Object.create(null) as NonNullable<PreviewAccess["credentials"]>;
      for (const { id: ref } of document.secretRefs) {
        const source = this.credentialSources().get(ref);
        const field = form.elements.namedItem(
          "secret-" + ref,
        ) as HTMLInputElement | null;
        if (!field || !field.value) continue;
        if (source === "session") {
          credentials[ref] = { kind: source, value: field.value };
          field.value = "";
        } else if (source === "environment") {
          credentials[ref] = { kind: source, name: field.value.trim() };
        }
      }
      access.credentials = credentials;
    }

    this.status.set("Preparing launch review…");
    this.setPhase("preparing");
    const reply = await desktop("policy_review", {
      subject,
      mode: selectedMode,
      access,
      protocol: this.protocol(),
      capturePayloads: this.capturePayloads(),
    });
    if (this.destroyed) return;
    this.diagnostics.set(diagnosticsFrom(reply));
    this.status.set("");
    if (reply.ok) {
      this.review.set({ subject, mode: selectedMode, details: reply.data });
      this.setPhase("review");
    } else {
      this.setPhase("configure");
    }
  }

  protected discardReview(): void {
    this.review.set(null);
    this.diagnostics.set([]);
    this.status.set("Launch review cancelled.");
    this.setPhase("configure");
  }

  protected async launch(): Promise<void> {
    const pending = this.review();
    if (!pending || this.phase() !== "review" || this.blocked()) return;
    this.review.set(null);
    this.setPhase("starting");
    this.status.set("Starting the local server…");
    this.diagnostics.set([]);
    this.result.set(null);
    this.trace.set(null);

    const approved = await desktop("policy_approve", {
      reviewId: pending.details.reviewId,
      digest: pending.details.digest,
    });
    if (this.destroyed) return;
    if (!approved.ok) {
      this.setPhase("configure");
      this.showFailure(diagnosticsFrom(approved), "Launch approval failed.");
      return;
    }
    const startup = await desktop("playground_start", {
      subject: pending.subject,
      approval: { approvalId: approved.data.approvalId },
    });
    if (!startup.ok) {
      if (this.destroyed) return;
      this.setPhase("configure");
      this.showFailure(
        diagnosticsFrom(startup),
        "The local server did not start.",
      );
      return;
    }
    const id = startup.data.operationId;
    if (this.destroyed) {
      void desktop("run_cancel", { id });
      return;
    }
    this.operationId.set(id);
    const started = await this.poll(id);
    this.operationId.set(null);
    if (this.destroyed) {
      if (started?.kind === "started")
        void desktop("playground_stop", { connectionId: started.connectionId });
      return;
    }
    if (started?.kind === "started") {
      this.connection.set({
        id: started.connectionId,
        mode: pending.mode,
        discovery: started.discovery,
      });
      this.setPhase("connected");
      this.status.set("Local server connected.");
    } else {
      this.setPhase("configure");
      if (started?.kind === "failed")
        this.showFailure([started.error], "The local server did not start.");
      else
        this.status.set(
          "The start operation did not report a connection. Review a new launch to retry.",
        );
    }
  }

  protected async cancel(id: string): Promise<void> {
    const reply = await desktop("run_cancel", { id });
    if (this.destroyed) return;
    this.diagnostics.set(diagnosticsFrom(reply));
    if (reply.ok)
      this.status.set(
        reply.data.cancellationRequested
          ? "Cancellation requested…"
          : "The operation has already finished.",
      );
  }

  protected async stop(): Promise<void> {
    const connection = this.connection();
    if (!connection || this.phase() !== "connected") return;
    this.setPhase("stopping");
    this.status.set("Stopping the local server…");
    const reply = await desktop("playground_stop", {
      connectionId: connection.id,
    });
    if (this.destroyed) return;
    if (!reply.ok && reply.error.code === "desktop.ipc_unavailable") {
      this.setPhase("connected");
      this.showFailure(
        diagnosticsFrom(reply),
        "Could not confirm server shutdown. Try Stop server again.",
      );
      return;
    }
    this.connection.set(null);
    this.result.set(null);
    this.trace.set(null);
    this.runId.set(null);
    this.runBusy.set(false);
    this.diagnostics.set(diagnosticsFrom(reply));
    this.status.set(
      reply.ok
        ? "Local server stopped."
        : "The connection closed with a shutdown error.",
    );
    this.setPhase("configure");
  }

  protected pickInvocation(method: Invocation["method"], target: string): void {
    this.method.set(method);
    this.target.set(target);
    this.argumentsText.set("{}");
  }

  protected changeMethod(event: Event): void {
    const method = this.controlValue(event);
    if (
      method === "tools/call" ||
      method === "resources/read" ||
      method === "prompts/get"
    ) {
      this.pickInvocation(method, "");
    }
  }

  protected changeTarget(event: Event): void {
    this.target.set(this.controlValue(event));
  }

  protected changeArguments(event: Event): void {
    this.argumentsText.set(this.controlValue(event));
  }

  protected changeTimeout(event: Event): void {
    this.timeout.set(this.controlValue(event));
  }

  protected async run(): Promise<void> {
    const connection = this.connection();
    if (!connection || this.phase() !== "connected" || this.runBusy()) return;
    const timeoutMs = this.timeout().trim()
      ? Number(this.timeout())
      : undefined;
    if (
      timeoutMs !== undefined &&
      (!Number.isInteger(timeoutMs) || timeoutMs < 1 || timeoutMs > 30_000)
    ) {
      this.status.set("Enter a timeout from 1 to 30,000 ms.");
      return;
    }
    const call = invocation(this.method(), this.target(), this.argumentsText());
    if (connection.mode.kind === "live" && !call) {
      this.status.set(
        this.method() === "prompts/get"
          ? "Enter a prompt name and a JSON object with string values."
          : "Enter a target and a JSON object for tool arguments.",
      );
      return;
    }
    const { registrationId, revision } = this.snapshot();
    this.result.set(null);
    this.diagnostics.set([]);
    this.runBusy.set(true);
    this.status.set(
      connection.mode.kind === "fixture"
        ? "Replaying the fixture…"
        : "Running the live request…",
    );
    try {
      const reply =
        connection.mode.kind === "fixture"
          ? await desktop("scenario_run", {
              connectionId: connection.id,
              registrationId,
              expectedRevision: revision,
              scenarioId: connection.mode.scenarioId,
              timeoutMs,
            })
          : await desktop("playground_call", {
              connectionId: connection.id,
              call: call!,
              timeoutMs,
            });
      if (this.destroyed || this.connection()?.id !== connection.id) {
        if (reply.ok) void desktop("run_cancel", { id: reply.data.runId });
        return;
      }
      if (!reply.ok) {
        this.showFailure(diagnosticsFrom(reply), "Could not run the request.");
        return;
      }
      this.runId.set(reply.data.runId);
      const event = await this.poll(reply.data.runId);
      if (
        this.destroyed ||
        this.connection()?.id !== connection.id ||
        this.phase() !== "connected"
      )
        return;
      if (event?.kind === "completed") {
        this.result.set(event.run);
        this.status.set(
          this.outcomeNames[event.run.result.outcome] +
            " in " +
            event.run.result.durationMs +
            " ms.",
        );
      } else if (event?.kind === "failed") {
        this.showFailure([event.error], "The request failed.");
      } else {
        this.status.set("The operation did not return a result.");
      }
      await this.refreshTrace();
    } finally {
      if (this.connection()?.id === connection.id) {
        this.runId.set(null);
        this.runBusy.set(false);
      }
    }
  }

  protected async refreshTrace(): Promise<void> {
    const connection = this.connection();
    if (!connection || this.phase() !== "connected" || this.traceBusy()) return;
    this.traceBusy.set(true);
    try {
      const reply = await desktop("playground_trace", {
        connectionId: connection.id,
      });
      if (
        this.destroyed ||
        this.connection()?.id !== connection.id ||
        this.phase() !== "connected"
      )
        return;
      if (reply.ok) this.trace.set(reply.data);
      else
        this.diagnostics.update((items) => [
          ...items,
          ...diagnosticsFrom(reply),
        ]);
    } finally {
      this.traceBusy.set(false);
    }
  }

  private async poll(id: string): Promise<RunEvent | null> {
    let afterCursor = 0;
    while (!this.destroyed) {
      const reply = await desktop("run_poll", { id, afterCursor });
      if (!reply.ok) {
        this.diagnostics.set(diagnosticsFrom(reply));
        void desktop("run_cancel", { id });
        return null;
      }
      if (reply.data.events.length) return reply.data.events[0];
      if (reply.data.terminal) return null;
      afterCursor = reply.data.nextCursor;
      await new Promise<void>((resolve) => setTimeout(resolve, 150));
    }
    return null;
  }

  private setPhase(phase: Phase): void {
    this.phase.set(phase);
    this.activeChange.emit(phase !== "configure");
  }

  private showFailure(diagnostics: Diagnostic[], status: string): void {
    this.diagnostics.set(diagnostics);
    this.status.set(status);
  }

  private controlValue(event: Event): string {
    return (
      event.target as HTMLInputElement | HTMLSelectElement | HTMLTextAreaElement
    ).value;
  }
}
