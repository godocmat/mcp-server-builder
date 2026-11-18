import {
  ChangeDetectionStrategy,
  Component,
  input,
  output,
} from "@angular/core";
import type { Diagnostic } from "./mvp-ipc";

@Component({
  selector: "app-diagnostics",
  changeDetection: ChangeDetectionStrategy.OnPush,
  template: `
    @if (diagnostics().length) {
      <ul class="diagnostics" aria-label="Diagnostics" aria-live="polite">
        @for (item of diagnostics(); track $index) {
          <li [class.warning]="item.severity === 'warning'">
            <strong>{{ item.message }}</strong>
            @if (item.action) {
              <span>{{ item.action }}</span>
            }
            <small
              >{{ item.code }}
              @if (item.field) {
                · <code>{{ item.field }}</code>
              }
            </small>
            @if (navigateFields() && item.field) {
              <button
                type="button"
                class="quiet-button"
                (click)="fieldSelected.emit(item)"
              >
                Go to field
              </button>
            }
          </li>
        }
      </ul>
    }
  `,
})
export class DiagnosticsComponent {
  readonly diagnostics = input.required<Diagnostic[]>();
  readonly navigateFields = input(false);
  readonly fieldSelected = output<Diagnostic>();
}
