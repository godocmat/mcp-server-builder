import { ChangeDetectionStrategy, Component, signal } from "@angular/core";
import { checkWorkspace, type Reply, type SmokeStatus } from "./smoke-ipc";

type SmokeState =
  | { kind: "idle" }
  | { kind: "running" }
  | { kind: "complete"; reply: Reply<SmokeStatus> }
  | { kind: "unavailable" };

@Component({
  selector: "app-workspace-smoke",
  templateUrl: "./workspace-smoke.component.html",
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class WorkspaceSmokeComponent {
  protected readonly state = signal<SmokeState>({ kind: "idle" });

  protected async run(): Promise<void> {
    this.state.set({ kind: "running" });
    try {
      const reply = await checkWorkspace();
      this.state.set({ kind: "complete", reply });
    } catch {
      this.state.set({ kind: "unavailable" });
    }
  }
}
