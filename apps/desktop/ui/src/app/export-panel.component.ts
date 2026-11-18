import {
  ChangeDetectionStrategy,
  Component,
  computed,
  input,
  signal,
} from "@angular/core";
import { DiagnosticsComponent } from "./diagnostics.component";
import {
  desktop,
  diagnosticsFrom,
  handle,
  type ConflictDecision,
  type Diagnostic,
  type GenerationPlan,
  type GenerationReceipt,
  type PathSelection,
} from "./mvp-ipc";
import type { ProjectSnapshot } from "./project-model";

@Component({
  selector: "app-export-panel",
  imports: [DiagnosticsComponent],
  templateUrl: "./export-panel.component.html",
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class ExportPanelComponent {
  readonly snapshot = input.required<ProjectSnapshot>();
  readonly dirty = input.required<boolean>();
  protected readonly parent = signal<PathSelection | null>(null);
  protected readonly previous = signal<PathSelection | null>(null);
  protected readonly folderName = signal("");
  protected readonly diagnostics = signal<Diagnostic[]>([]);
  protected readonly status = signal("");
  protected readonly busy = signal(false);
  private readonly plan = signal<{
    result: GenerationPlan;
    revision: string;
  } | null>(null);
  private readonly lastExport = signal<{
    receipt: GenerationReceipt;
    revision: string;
  } | null>(null);
  protected readonly decisions = signal<ReadonlyMap<string, ConflictDecision>>(
    new Map(),
  );
  protected readonly activePlan = computed(() => {
    const plan = this.plan();
    return plan?.revision === this.snapshot().revision && !this.dirty()
      ? plan.result
      : null;
  });
  protected readonly receipt = computed(() => {
    const result = this.lastExport();
    return result?.revision === this.snapshot().revision && !this.dirty()
      ? result.receipt
      : null;
  });
  protected readonly canWrite = computed(
    () =>
      this.activePlan()?.conflicts.every((item) =>
        this.decisions().has(item.path),
      ) ?? false,
  );

  protected changeFolderName(event: Event): void {
    this.folderName.set((event.target as HTMLInputElement).value);
    this.clearPlan();
  }

  protected async chooseFolder(type: "parent" | "previous"): Promise<void> {
    if (this.busy()) return;
    this.busy.set(true);
    try {
      const reply = await desktop("select_path", {
        purpose: type === "parent" ? "exportParent" : "previousExport",
      });
      this.diagnostics.set(diagnosticsFrom(reply));
      if (reply.ok && reply.data) {
        (type === "parent" ? this.parent : this.previous).set(reply.data);
        this.clearPlan();
      }
    } finally {
      this.busy.set(false);
    }
  }

  protected clearPrevious(): void {
    this.previous.set(null);
    this.clearPlan();
  }

  protected async preview(): Promise<void> {
    const parent = this.parent();
    const fromExport = this.previous();
    if (!parent || this.busy() || this.dirty() || !this.folderName().trim())
      return;
    const { registrationId, revision } = this.snapshot();
    this.clearPlan();
    this.busy.set(true);
    try {
      this.parent.set(null);
      this.previous.set(null);
      const reply = await desktop("generation_plan", {
        registrationId,
        expectedRevision: revision,
        destination: handle(parent),
        folderName: this.folderName().trim(),
        ...(fromExport ? { fromExport: handle(fromExport) } : {}),
      });
      this.diagnostics.set(diagnosticsFrom(reply));
      if (reply.ok) this.plan.set({ result: reply.data, revision });
    } finally {
      this.busy.set(false);
    }
  }

  protected chooseDecision(path: string, event: Event): void {
    const decision = (event.target as HTMLSelectElement).value;
    const next = new Map(this.decisions());
    if (decision === "keep" || decision === "regenerate")
      next.set(path, decision);
    else next.delete(path);
    this.decisions.set(next);
  }

  protected async write(): Promise<void> {
    const plan = this.activePlan();
    if (!plan || !this.canWrite() || this.busy()) return;
    const revision = this.snapshot().revision;
    this.busy.set(true);
    this.plan.set(null);
    try {
      const choices = this.decisions();
      const reply = await desktop("generation_write", {
        planId: plan.planId,
        planDigest: plan.planDigest,
        resolutions: plan.conflicts.map(
          ({ path, observedDigest, replacementDigest }) => ({
            path,
            observedDigest,
            replacementDigest,
            decision: choices.get(path)!,
          }),
        ),
      });
      this.diagnostics.set(diagnosticsFrom(reply));
      if (reply.ok) this.lastExport.set({ receipt: reply.data, revision });
    } finally {
      this.busy.set(false);
    }
  }

  protected async copyCommand(): Promise<void> {
    const output = this.receipt();
    if (!output) return;
    const path = output.destinationDisplayPath.replaceAll("'", "''");
    try {
      await navigator.clipboard.writeText(
        "Set-Location -LiteralPath '" + path + "'\ncargo build --locked",
      );
      this.status.set("Build command copied.");
    } catch {
      this.status.set(
        "Copy the folder path and build command from the instructions above.",
      );
    }
  }

  private clearPlan(): void {
    this.plan.set(null);
    this.lastExport.set(null);
    this.decisions.set(new Map());
    this.diagnostics.set([]);
    this.status.set("");
  }
}
