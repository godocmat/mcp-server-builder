import {
  ChangeDetectionStrategy,
  Component,
  computed,
  signal,
} from "@angular/core";
import { DiagnosticsComponent } from "./diagnostics.component";
import {
  desktop,
  diagnosticsFrom,
  handle,
  type ClientConfigPreview,
  type ClientConfigTarget,
  type Diagnostic,
  type PathPurpose,
  type PathSelection,
} from "./mvp-ipc";

type FileField = "executable" | "manifest" | "runtimeConfig";

const filePurposes = {
  executable: "clientExecutable",
  manifest: "clientManifest",
  runtimeConfig: "clientRuntimeConfig",
} as const satisfies Record<FileField, PathPurpose>;

@Component({
  selector: "app-client-config-panel",
  imports: [DiagnosticsComponent],
  templateUrl: "./client-config-panel.component.html",
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class ClientConfigPanelComponent {
  protected readonly target = signal<ClientConfigTarget>("codex");
  protected readonly name = signal("");
  protected readonly files = signal<Record<FileField, PathSelection | null>>({
    executable: null,
    manifest: null,
    runtimeConfig: null,
  });
  protected readonly output = signal<ClientConfigPreview | null>(null);
  protected readonly diagnostics = signal<Diagnostic[]>([]);
  protected readonly busy = signal(false);
  protected readonly status = signal("");
  protected readonly ready = computed(
    () =>
      !!(
        this.name().trim() &&
        this.files().executable &&
        this.files().manifest
      ),
  );
  protected readonly instructions: Record<ClientConfigTarget, string> = {
    "claude-code":
      "Merge the server entry into the mcpServers object in your project's .mcp.json file.",
    codex: "Add the server table to your Codex config.toml file.",
    opencode:
      "Merge the server entry into mcp.servers in your OpenCode V2 configuration file.",
    generic:
      "Save the snippet as a new file and load it in MCP Inspector with --config and --server.",
  };

  protected changeTarget(event: Event): void {
    this.target.set(
      (event.target as HTMLSelectElement).value as ClientConfigTarget,
    );
    this.resetOutput();
  }

  protected changeName(event: Event): void {
    this.name.set((event.target as HTMLInputElement).value);
    this.resetOutput();
  }

  protected async choose(field: FileField): Promise<void> {
    if (this.busy()) return;
    this.busy.set(true);
    try {
      const reply = await desktop("select_path", {
        purpose: filePurposes[field],
      });
      if (reply.ok && reply.data) {
        const selection = reply.data;
        this.files.update((value) => ({ ...value, [field]: selection }));
        this.resetOutput();
      }
      this.diagnostics.set(diagnosticsFrom(reply));
    } finally {
      this.busy.set(false);
    }
  }

  protected clearRuntimeConfig(): void {
    this.files.update((value) => ({ ...value, runtimeConfig: null }));
    this.resetOutput();
  }

  protected async generate(): Promise<void> {
    const { executable, manifest, runtimeConfig } = this.files();
    if (!executable || !manifest || !this.ready() || this.busy()) return;
    this.resetOutput();
    this.busy.set(true);
    try {
      const reply = await desktop("client_config_export", {
        target: this.target(),
        name: this.name().trim(),
        executable: handle(executable),
        manifest: handle(manifest),
        ...(runtimeConfig ? { runtimeConfig: handle(runtimeConfig) } : {}),
      });
      this.diagnostics.set(diagnosticsFrom(reply));
      if (reply.ok) {
        this.output.set(reply.data);
        this.status.set("Client configuration generated.");
      }
    } finally {
      this.busy.set(false);
    }
  }

  protected async copy(text: HTMLTextAreaElement): Promise<void> {
    const output = this.output();
    if (!output || this.busy()) return;
    this.busy.set(true);
    try {
      await navigator.clipboard.writeText(output.contents);
      this.status.set("Client configuration copied.");
    } catch {
      text.focus();
      text.select();
      this.status.set("Client configuration selected for manual copying.");
    } finally {
      this.busy.set(false);
    }
  }

  protected async save(): Promise<void> {
    const output = this.output();
    if (!output || this.busy()) return;
    this.busy.set(true);
    this.status.set("");
    try {
      const reply = await desktop("client_config_save", {
        configurationId: output.configurationId,
      });
      this.diagnostics.set(diagnosticsFrom(reply));
      if (reply.ok && reply.data?.destination)
        this.status.set(
          "Saved client configuration to " + reply.data.destination,
        );
    } finally {
      this.busy.set(false);
    }
  }

  private resetOutput(): void {
    this.output.set(null);
    this.diagnostics.set([]);
    this.status.set("");
  }
}
