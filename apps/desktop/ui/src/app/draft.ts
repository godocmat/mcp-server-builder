import type { ProjectDocument } from "./project-model";

export type FieldFormat =
  "text" | "longText" | "number" | "json" | "toggle" | "select";

export interface EditorField {
  path: string;
  label: string;
  format?: FieldFormat;
  help?: string;
  optional?: boolean;
  choices?: string[];
}

export interface ProjectDraft {
  document: ProjectDocument;
  input: Readonly<Record<string, string>>;
  syntaxErrors: Readonly<Record<string, string>>;
}

export interface FieldChange {
  field: EditorField;
  value: string | boolean;
}

export function draftFor(document: ProjectDocument): ProjectDraft {
  return { document, input: {}, syntaxErrors: {} };
}

export function pathValue(document: ProjectDocument, path: string): unknown {
  let value: unknown = document;
  for (const segment of path.split("/").slice(1)) {
    if (
      typeof value !== "object" ||
      value === null ||
      !Object.hasOwn(value, segment)
    )
      return undefined;
    value = (value as Record<string, unknown>)[segment];
  }
  return value;
}

export function fieldText(draft: ProjectDraft, field: EditorField): string {
  if (Object.hasOwn(draft.input, field.path)) return draft.input[field.path];
  const value = pathValue(draft.document, field.path);
  if (value === undefined) return "";
  return field.format === "json"
    ? JSON.stringify(value, null, 2)
    : String(value);
}

export function editField(
  draft: ProjectDraft,
  { field, value }: FieldChange,
): ProjectDraft {
  const input = { ...draft.input };
  const syntaxErrors = { ...draft.syntaxErrors };
  delete syntaxErrors[field.path];

  let data: unknown = value;
  if (typeof value === "string") {
    if (field.optional && value.trim() === "") {
      data = undefined;
    } else if (field.format === "number") {
      const parsed = Number(value);
      if (!/^\d+$/.test(value) || !Number.isSafeInteger(parsed)) {
        syntaxErrors[field.path] = "Enter a whole number of zero or more.";
      } else {
        data = parsed;
      }
    } else if (field.format === "json") {
      try {
        data = JSON.parse(value) as unknown;
      } catch {
        syntaxErrors[field.path] = "Enter valid JSON.";
      }
    }
  }

  if (field.format === "json" || field.format === "number") {
    input[field.path] = String(value);
  }
  if (syntaxErrors[field.path]) return { ...draft, input, syntaxErrors };

  const document = structuredClone(draft.document);
  const parts = field.path.split("/").slice(1);
  const key = parts.pop()!;
  let record = document as unknown as Record<string, unknown>;
  for (const part of parts) {
    if (!Object.hasOwn(record, part)) record[part] = {};
    record = record[part] as Record<string, unknown>;
  }
  if (data === undefined) {
    // Omit an empty optional object (for example, grant limits) as well as its field.
    delete record[key];
  } else {
    record[key] = data;
  }

  if (parts.at(-1) === "limits" && Object.keys(record).length === 0) {
    const parent = pathValue(document, "/" + parts.slice(0, -1).join("/"));
    delete (parent as { limits?: unknown }).limits;
  }

  return { document, input, syntaxErrors };
}

export function syntaxMessage(draft: ProjectDraft): string | undefined {
  return Object.values(draft.syntaxErrors)[0];
}

export function fieldId(path: string): string {
  return "editor-field-" + path;
}
