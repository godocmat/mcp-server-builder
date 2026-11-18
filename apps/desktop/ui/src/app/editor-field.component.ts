import {
  ChangeDetectionStrategy,
  Component,
  computed,
  input,
  output,
} from "@angular/core";
import {
  fieldId,
  fieldText,
  pathValue,
  type EditorField,
  type FieldChange,
  type ProjectDraft,
} from "./draft";
import type { Diagnostic } from "./mvp-ipc";

@Component({
  selector: "app-editor-field",
  templateUrl: "./editor-field.component.html",
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class EditorFieldComponent {
  readonly draft = input.required<ProjectDraft>();
  readonly field = input.required<EditorField>();
  readonly diagnostics = input.required<Diagnostic[]>();
  readonly fieldEdited = output<FieldChange>();

  protected readonly id = computed(() => fieldId(this.field().path));
  protected readonly text = computed(() =>
    fieldText(this.draft(), this.field()),
  );
  protected readonly checked = computed(
    () => pathValue(this.draft().document, this.field().path) === true,
  );
  protected readonly error = computed(() => {
    const { path } = this.field();
    const local = this.draft().syntaxErrors[path];
    if (local) return local;
    return this.diagnostics().find(
      (issue) =>
        issue.severity === "error" &&
        (issue.field === path || issue.field?.startsWith(path + "/")),
    )?.message;
  });

  protected edit(event: Event): void {
    const control = event.target as
      HTMLInputElement | HTMLSelectElement | HTMLTextAreaElement;
    const field = this.field();
    this.fieldEdited.emit({
      field,
      value:
        field.format === "toggle"
          ? (control as HTMLInputElement).checked
          : control.value,
    });
  }
}
