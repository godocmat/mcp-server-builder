import { DOCUMENT } from "@angular/common";
import {
  ChangeDetectionStrategy,
  Injector,
  afterNextRender,
  inject,
  Component,
  computed,
  signal,
} from "@angular/core";
import { DiagnosticsComponent } from "./diagnostics.component";
import { draftFor, syntaxMessage, type ProjectDraft } from "./draft";
import { ExportPanelComponent } from "./export-panel.component";
import {
  desktop,
  diagnosticsFrom,
  handle,
  type Diagnostic,
  type PathSelection,
  type ProjectRoot,
  type RecoveryCandidate,
  type RecoveryInfo,
} from "./mvp-ipc";
import { PlaygroundComponent } from "./playground.component";
import { ProjectEditorComponent } from "./project-editor.component";
import type { ProjectSnapshot, ProjectSummary } from "./project-model";
import { WorkspaceSmokeComponent } from "./workspace-smoke.component";

type Section = "projects" | "editor" | "playground" | "export";
type Recovery = RecoveryInfo & {
  registrationId?: string;
  sourceDisplayPath?: string;
};

function recoveryFrom(diagnostics: Diagnostic[]): RecoveryInfo | null {
  const details = diagnostics.find(
    (item) => item.code === "project.recovery_available",
  )?.details;
  if (
    !details ||
    typeof details !== "object" ||
    !("candidates" in details) ||
    !Array.isArray(details.candidates)
  )
    return null;
  const currentRevision =
    "currentRevision" in details && typeof details.currentRevision === "string"
      ? details.currentRevision
      : null;
  return {
    currentRevision,
    candidates: details.candidates as RecoveryCandidate[],
  };
}

@Component({
  selector: "app-workbench",
  imports: [
    DiagnosticsComponent,
    ExportPanelComponent,
    PlaygroundComponent,
    ProjectEditorComponent,
    WorkspaceSmokeComponent,
  ],
  templateUrl: "./workbench.component.html",
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class WorkbenchComponent {
  private readonly injector = inject(Injector);
  private readonly page = inject(DOCUMENT);
  protected readonly section = signal<Section>("projects");
  protected readonly snapshot = signal<ProjectSnapshot | null>(null);
  protected readonly draft = signal<ProjectDraft | null>(null);
  protected readonly status = signal("");
  protected readonly projectDiagnostics = signal<Diagnostic[]>([]);
  protected readonly catalogDiagnostics = signal<Diagnostic[]>([]);
  protected readonly editorDiagnostics = signal<Diagnostic[]>([]);
  protected readonly projects = signal<ProjectSummary[]>([]);
  protected readonly catalogBusy = signal(false);
  protected readonly busy = signal(false);
  protected readonly previewActive = signal(false);
  protected readonly creating = signal(false);
  protected readonly newName = signal("");
  protected readonly destination = signal<PathSelection | null>(null);
  protected readonly conflict = signal<string | null>(null);
  protected readonly recovery = signal<Recovery | null>(null);
  protected readonly pending = signal<(() => Promise<void>) | null>(null);
  protected readonly dirty = computed(() => {
    const draft = this.draft();
    return (
      draft !== null &&
      (draft.document !== this.snapshot()?.document ||
        Object.keys(draft.input).length > 0)
    );
  });
  protected readonly mounted = computed(() => {
    const project = this.snapshot();
    return project ? [project] : [];
  });

  constructor() {
    void this.refreshProjects();
  }

  protected selectSection(section: Section): void {
    this.pending.set(null);
    this.section.set(section);
    const heading = {
      projects: "projects-heading",
      editor: "project-heading",
      playground: "playground-heading",
      export: "export-heading",
    }[section];
    this.focus(heading);
  }

  protected async refreshProjects(): Promise<void> {
    if (this.catalogBusy()) return;
    this.catalogBusy.set(true);
    try {
      const reply = await desktop("project_list", {});
      if (reply.ok) this.projects.set(reply.data);
      this.catalogDiagnostics.set(diagnosticsFrom(reply));
    } finally {
      this.catalogBusy.set(false);
    }
  }

  protected showCreate(): void {
    this.creating.set(true);
    this.focus("new-project-name");
  }

  protected changeNewName(event: Event): void {
    this.newName.set((event.target as HTMLInputElement).value);
  }

  protected async chooseDestination(): Promise<void> {
    if (this.busy()) return;
    await this.work(async () => {
      const reply = await desktop("select_path", {
        purpose: "projectDestination",
      });
      this.projectDiagnostics.set(diagnosticsFrom(reply));
      if (reply.ok && reply.data) this.destination.set(reply.data);
    });
  }

  protected openFolder(): void {
    this.confirmNavigation(async () => {
      const chosen = await desktop("select_path", { purpose: "projectRoot" });
      if (!chosen.ok) this.projectDiagnostics.set(diagnosticsFrom(chosen));
      if (chosen.ok && chosen.data)
        await this.open(handle(chosen.data), chosen.data.displayPath);
    });
  }

  protected openRegistration(registrationId: string): void {
    this.confirmNavigation(() => this.open({ registrationId }));
  }

  protected create(): void {
    const destination = this.destination();
    const name = this.newName().trim();
    if (!destination || !name) return;
    this.confirmNavigation(async () => {
      this.destination.set(null);
      const reply = await desktop("project_create", {
        destination: handle(destination),
        name,
      });
      this.projectDiagnostics.set(diagnosticsFrom(reply));
      if (reply.ok) {
        this.opened(reply.data, reply.diagnostics, "Project created.");
        this.creating.set(false);
        this.newName.set("");
        this.destination.set(null);
      }
    });
  }

  protected async confirm(): Promise<void> {
    const action = this.pending();
    if (!action || this.busy() || this.previewActive()) return;
    this.pending.set(null);
    await this.work(action);
  }

  protected changeDraft(draft: ProjectDraft): void {
    this.draft.set(draft);
    this.editorDiagnostics.set([]);
    this.status.set("");
  }

  protected async validate(): Promise<void> {
    const draft = this.checkedDraft();
    if (!draft || this.busy()) return;
    await this.work(async () => {
      const reply = await desktop("project_validate", {
        document: draft.document,
      });
      this.editorDiagnostics.set(diagnosticsFrom(reply));
      this.status.set(
        reply.ok ? "The draft is valid." : "Review the validation errors.",
      );
    });
  }

  protected async save(): Promise<void> {
    const current = this.snapshot();
    const draft = this.checkedDraft();
    if (!current || !draft || this.busy() || this.previewActive()) return;
    await this.work(async () => {
      const reply = await desktop("project_save", {
        registrationId: current.registrationId,
        expectedRevision: current.revision,
        document: draft.document,
      });
      if (reply.ok) {
        this.opened(reply.data, reply.diagnostics, "Project saved.");
      } else {
        const diagnostics = diagnosticsFrom(reply);
        this.editorDiagnostics.set(diagnostics);
        this.status.set("Project was not saved.");
        this.setRecovery(diagnostics, current.registrationId);
        if (reply.error.code === "project.revision_conflict") {
          const details = reply.error.details;
          this.conflict.set(
            details &&
              typeof details === "object" &&
              "currentRevision" in details &&
              typeof details.currentRevision === "string"
              ? details.currentRevision
              : "Unavailable",
          );
        }
      }
    });
  }

  protected reloadSaved(): void {
    const current = this.snapshot();
    if (!current) return;
    this.confirmNavigation(() =>
      this.open({ registrationId: current.registrationId }),
    );
  }

  protected async saveDraftCopy(): Promise<void> {
    const current = this.snapshot();
    const draft = this.checkedDraft();
    if (!current || !draft || this.busy() || this.previewActive()) return;
    await this.work(async () => {
      const chosen = await desktop("select_path", {
        purpose: "recoveryDestination",
      });
      if (!chosen.ok) this.editorDiagnostics.set(diagnosticsFrom(chosen));
      if (!chosen.ok || !chosen.data) return;
      const reply = await desktop("project_recover", {
        registrationId: current.registrationId,
        source: { kind: "draft", document: draft.document },
        destination: handle(chosen.data),
      });
      this.editorDiagnostics.set(diagnosticsFrom(reply));
      if (reply.ok)
        this.opened(
          reply.data,
          reply.diagnostics,
          "Draft saved as a new project.",
        );
    });
  }

  protected restoreBackup(
    recovery: Recovery,
    candidate: RecoveryCandidate,
  ): void {
    this.confirmNavigation(async () => {
      const messages =
        this.section() === "projects"
          ? this.projectDiagnostics
          : this.editorDiagnostics;
      let root: ProjectRoot;
      if (recovery.registrationId) {
        root = { registrationId: recovery.registrationId };
      } else {
        const chosen = await desktop("select_path", { purpose: "projectRoot" });
        if (!chosen.ok) messages.set(diagnosticsFrom(chosen));
        if (!chosen.ok || !chosen.data) return;
        if (chosen.data.displayPath !== recovery.sourceDisplayPath) {
          this.status.set(
            "Select the project folder that reported this recovery file.",
          );
          return;
        }
        root = handle(chosen.data);
      }
      const reply = await desktop("project_recover", {
        ...("registrationId" in root ? root : { root }),
        expectedRevision: recovery.currentRevision,
        source: { kind: "backup", revision: candidate.revision },
      });
      messages.set(diagnosticsFrom(reply));
      if (reply.ok) {
        this.opened(
          reply.data,
          reply.diagnostics,
          "Project restored from the selected revision.",
        );
      } else {
        this.setRecovery(
          diagnosticsFrom(reply),
          recovery.registrationId,
          recovery.sourceDisplayPath,
        );
      }
    }, true);
  }

  private checkedDraft(): ProjectDraft | null {
    const draft = this.draft();
    if (!draft) return null;
    if (syntaxMessage(draft)) {
      this.editorDiagnostics.set(
        Object.entries(draft.syntaxErrors).map(([field, message]) => ({
          code: "input.invalid",
          severity: "error",
          correlationId: "",
          field,
          message,
        })),
      );
      this.status.set("Fix the highlighted fields.");
      return null;
    }
    return draft;
  }

  private async open(root: ProjectRoot, path?: string): Promise<void> {
    const reply = await desktop("project_open", { root });
    this.setRecovery(
      diagnosticsFrom(reply),
      "registrationId" in root ? root.registrationId : undefined,
      path,
    );
    if (reply.ok) {
      this.opened(reply.data, reply.diagnostics, "Project opened.");
    } else {
      // Leave any open draft alone. Show the failed location and its recovery choices first.
      this.section.set("projects");
      this.projectDiagnostics.set(diagnosticsFrom(reply));
      this.status.set("Could not open the project.");
    }
  }

  private opened(
    project: ProjectSnapshot,
    diagnostics: Diagnostic[],
    status: string,
  ): void {
    this.snapshot.set(project);
    this.draft.set(draftFor(project.document));
    this.editorDiagnostics.set(diagnostics);
    this.recovery.set(null);
    this.conflict.set(null);
    if (this.section() !== "editor") this.selectSection("editor");
    this.status.set(status + " " + project.rootDisplayPath);
    void this.refreshProjects();
  }

  private setRecovery(
    diagnostics: Diagnostic[],
    registrationId?: string,
    sourceDisplayPath?: string,
  ): void {
    const recovery = recoveryFrom(diagnostics);
    this.recovery.set(
      recovery ? { ...recovery, registrationId, sourceDisplayPath } : null,
    );
  }

  private confirmNavigation(
    action: () => Promise<void>,
    alwaysConfirm = false,
  ): void {
    if (this.busy() || this.previewActive()) return;
    if (alwaysConfirm || this.dirty()) {
      this.pending.set(action);
      this.focus("navigation-confirmation");
    } else {
      void this.work(action);
    }
  }

  private focus(id: string): void {
    afterNextRender(() => this.page.getElementById(id)?.focus(), {
      injector: this.injector,
    });
  }

  private async work(action: () => Promise<void>): Promise<void> {
    this.busy.set(true);
    this.status.set("");
    try {
      await action();
    } finally {
      this.busy.set(false);
    }
  }
}
