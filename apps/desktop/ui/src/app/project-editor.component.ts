import { DOCUMENT } from "@angular/common";
import {
  ChangeDetectionStrategy,
  Component,
  Injector,
  afterNextRender,
  computed,
  inject,
  input,
  output,
  signal,
} from "@angular/core";
import { DiagnosticsComponent } from "./diagnostics.component";
import {
  editField,
  fieldId,
  type FieldChange,
  type ProjectDraft,
} from "./draft";
import { EditorFieldComponent } from "./editor-field.component";
import type { Diagnostic } from "./mvp-ipc";
import {
  bindingFields,
  capabilityFields,
  grantFields,
  projectFields,
  promptArgumentFields,
  promptMessageFields,
  scenarioFields,
  secretFields,
} from "./project-fields";
import {
  addCapability,
  addGrant,
  addScenario,
  capabilityLabels,
  type Capability,
  type Invocation,
} from "./project-model";

type View =
  | { kind: "project" | "secrets" }
  | { kind: "capability" | "scenario"; index: number };
type Removal = { parent: string; index: number };

function shiftAfterRemoval<T>(
  record: Readonly<Record<string, T>>,
  { parent, index }: Removal,
): Record<string, T> {
  const entries: [string, T][] = [];
  for (const [path, value] of Object.entries(record)) {
    if (!path.startsWith(parent + "/")) {
      entries.push([path, value]);
      continue;
    }
    const tail = path.slice(parent.length + 1);
    const separator = tail.indexOf("/");
    const row = Number(separator < 0 ? tail : tail.slice(0, separator));
    if (row === index) continue;
    const key =
      row < index
        ? path
        : parent +
          "/" +
          (row - 1) +
          (separator < 0 ? "" : tail.slice(separator));
    entries.push([key, value]);
  }
  return Object.fromEntries(entries);
}

@Component({
  selector: "app-project-editor",
  imports: [DiagnosticsComponent, EditorFieldComponent],
  templateUrl: "./project-editor.component.html",
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class ProjectEditorComponent {
  readonly draft = input.required<ProjectDraft>();
  readonly diagnostics = input.required<Diagnostic[]>();
  readonly busy = input(false);
  readonly draftChanged = output<ProjectDraft>();

  private readonly injector = inject(Injector);
  private readonly page = inject(DOCUMENT);
  protected readonly view = signal<View>({ kind: "project" });
  protected readonly project = computed(() => this.draft().document);
  protected readonly capabilityLabels = capabilityLabels;
  protected readonly projectFields = projectFields;
  protected readonly methodOptions: {
    value: Invocation["method"];
    label: string;
  }[] = [
    { value: "tools/call", label: "Call a tool" },
    { value: "resources/read", label: "Read a resource" },
    { value: "prompts/get", label: "Get a prompt" },
  ];

  protected readonly capabilityState = computed(() => {
    const view = this.view();
    if (view.kind !== "capability") return;
    const project = this.project();
    const capability = project.capabilities[view.index];
    if (!capability) return;
    const bindingIndex =
      "bindingId" in capability
        ? project.bindings.findIndex(
            (binding) => binding.id === capability.bindingId,
          )
        : -1;
    const binding = project.bindings[bindingIndex];
    const grants = project.policy.grants.flatMap((grant, index) =>
      grant.capabilityId === capability.id
        ? [{ grant, index, fields: grantFields(grant, index) }]
        : [],
    );
    return {
      capability,
      binding,
      bindingIndex,
      grants,
      index: view.index,
      fields: capabilityFields(capability, view.index),
      bindingFields: binding ? bindingFields(binding, bindingIndex) : [],
      arguments: promptArgumentFields(capability, view.index),
      messages: promptMessageFields(capability, view.index),
    };
  });

  protected readonly scenarioState = computed(() => {
    const view = this.view();
    if (view.kind !== "scenario") return;
    const scenario = this.project().scenarios[view.index];
    return scenario
      ? {
          scenario,
          index: view.index,
          fields: scenarioFields(scenario, view.index),
        }
      : undefined;
  });

  protected readonly secrets = computed(() =>
    secretFields(this.project().secretRefs),
  );

  protected changeField(edit: FieldChange): void {
    this.draftChanged.emit(editField(this.draft(), edit));
  }

  protected addCapability(kind: Capability["kind"]): void {
    this.update(addCapability(this.project(), kind));
    this.view.set({
      kind: "capability",
      index: this.project().capabilities.length,
    });
  }

  protected removeCapability(): void {
    const active = this.capabilityState();
    if (!active) return;
    const document = structuredClone(this.project());
    const removals: Removal[] = [
      { parent: "/capabilities", index: active.index },
    ];
    document.capabilities.splice(active.index, 1);
    for (const { index } of [...active.grants].reverse()) {
      document.policy.grants.splice(index, 1);
      removals.push({ parent: "/policy/grants", index });
    }
    if (
      active.binding &&
      !document.capabilities.some(
        (capability) =>
          "bindingId" in capability &&
          capability.bindingId === active.binding?.id,
      )
    ) {
      document.bindings.splice(active.bindingIndex, 1);
      removals.push({ parent: "/bindings", index: active.bindingIndex });
    }
    this.update(document, removals);
    this.view.set({ kind: "project" });
  }

  protected addGrant(): void {
    const active = this.capabilityState();
    if (active?.binding && "bindingId" in active.capability) {
      this.update(addGrant(this.project(), active.capability, active.binding));
    }
  }

  protected removeGrant(index: number): void {
    const document = structuredClone(this.project());
    document.policy.grants.splice(index, 1);
    this.update(document, [{ parent: "/policy/grants", index }]);
  }

  protected editPromptRows(
    group: "arguments" | "messages",
    removeIndex?: number,
  ): void {
    const active = this.capabilityState();
    if (!active) return;
    const document = structuredClone(this.project());
    const capability = document.capabilities[active.index];
    if (capability.kind !== "prompt") return;
    if (removeIndex !== undefined) {
      capability[group].splice(removeIndex, 1);
    } else if (group === "arguments") {
      capability.arguments.push({ name: "", required: true });
    } else {
      capability.messages.push({ role: "user", textTemplate: "" });
    }
    const parent = "/capabilities/" + active.index + "/" + group;
    this.update(
      document,
      removeIndex === undefined ? [] : [{ parent, index: removeIndex }],
    );
  }

  protected editSecretRows(removeIndex?: number): void {
    const document = structuredClone(this.project());
    if (removeIndex === undefined) {
      document.secretRefs.push({ id: "", purpose: "" });
    } else {
      document.secretRefs.splice(removeIndex, 1);
    }
    this.update(
      document,
      removeIndex === undefined
        ? []
        : [{ parent: "/secretRefs", index: removeIndex }],
    );
  }

  protected addScenario(): void {
    this.update(addScenario(this.project()));
    this.view.set({ kind: "scenario", index: this.project().scenarios.length });
  }

  protected removeScenario(index: number): void {
    const document = structuredClone(this.project());
    document.scenarios.splice(index, 1);
    this.update(document, [{ parent: "/scenarios", index }]);
    this.view.set({ kind: "project" });
  }

  protected selectScenarioMethod(event: Event): void {
    const active = this.scenarioState();
    if (!active) return;
    const target = (event.target as HTMLSelectElement).value;
    const document = structuredClone(this.project());
    const scenario = document.scenarios[active.index];
    switch (target) {
      case "tools/call":
        scenario.call = {
          method: target,
          name:
            document.capabilities.find((item) => item.kind === "tool")?.name ??
            "",
          arguments: {},
        };
        break;
      case "resources/read": {
        const resource = document.capabilities.find(
          (item) => item.kind === "resource",
        );
        scenario.call = { method: target, uri: resource?.uri ?? "" };
        break;
      }
      case "prompts/get":
        scenario.call = {
          method: target,
          name:
            document.capabilities.find((item) => item.kind === "prompt")
              ?.name ?? "",
          arguments: {},
        };
        break;
      default:
        return;
    }
    const argumentPath = "/scenarios/" + active.index + "/call/arguments";
    const { input, syntaxErrors } = this.draft();
    this.draftChanged.emit({
      document,
      input: Object.fromEntries(
        Object.entries(input).filter(([path]) => path !== argumentPath),
      ),
      syntaxErrors: Object.fromEntries(
        Object.entries(syntaxErrors).filter(([path]) => path !== argumentPath),
      ),
    });
  }

  protected selectDiagnostic(diagnostic: Diagnostic): void {
    const pointer = diagnostic.field;
    if (!pointer) return;
    const [, collection, row] = pointer.split("/");
    const document = this.project();
    switch (collection) {
      case "capabilities":
        this.view.set({ kind: "capability", index: Number(row) });
        break;
      case "bindings": {
        const bindingId = document.bindings[Number(row)]?.id;
        const index = document.capabilities.findIndex(
          (item) => "bindingId" in item && item.bindingId === bindingId,
        );
        if (index >= 0) this.view.set({ kind: "capability", index });
        break;
      }
      case "policy": {
        const index = Number(pointer.split("/")[3]);
        const capabilityId = document.policy.grants[index]?.capabilityId;
        const capability = document.capabilities.findIndex(
          (item) => item.id === capabilityId,
        );
        if (capability >= 0)
          this.view.set({ kind: "capability", index: capability });
        break;
      }
      case "scenarios":
        this.view.set({ kind: "scenario", index: Number(row) });
        break;
      case "secretRefs":
        this.view.set({ kind: "secrets" });
        break;
      default:
        this.view.set({ kind: "project" });
    }
    afterNextRender(
      () => {
        let path = pointer;
        while (path) {
          const field = this.page.getElementById(fieldId(path));
          if (field) {
            field.focus();
            return;
          }
          path = path.slice(0, path.lastIndexOf("/"));
        }
        this.page.getElementById("editor-section-heading")?.focus();
      },
      { injector: this.injector },
    );
  }

  private update(
    document: ProjectDraft["document"],
    removals: Removal[] = [],
  ): void {
    let { input, syntaxErrors } = this.draft();
    for (const remove of removals) {
      input = shiftAfterRemoval(input, remove);
      syntaxErrors = shiftAfterRemoval(syntaxErrors, remove);
    }
    this.draftChanged.emit({ document, input, syntaxErrors });
  }
}
