import type { EditorField } from "./draft";
import type {
  Binding,
  Capability,
  Grant,
  Scenario,
  SecretRef,
} from "./project-model";

const optional = { optional: true } as const;
const json = { format: "json" } as const;
const number = { format: "number" } as const;
const longText = { format: "longText" } as const;
const httpMethods = [
  "GET",
  "HEAD",
  "POST",
  "PUT",
  "PATCH",
  "DELETE",
  "OPTIONS",
];

export const projectFields: EditorField[] = [
  { path: "/name", label: "Project name" },
  { path: "/server/name", label: "Server name" },
  {
    path: "/server/version",
    label: "Server version",
    help: "Use a version such as 0.1.0.",
  },
  {
    path: "/server/description",
    label: "Server description",
    ...optional,
    ...longText,
  },
  {
    path: "/server/instructions",
    label: "Server instructions",
    ...optional,
    ...longText,
  },
];

type SuffixField = Omit<EditorField, "path"> & { key: string };

function fields(prefix: string, metadata: SuffixField[]): EditorField[] {
  return metadata.map(({ key, ...field }) => ({
    ...field,
    path: prefix + "/" + key,
  }));
}

export function capabilityFields(
  capability: Capability,
  index: number,
): EditorField[] {
  const base: SuffixField[] = [
    { key: "name", label: "Name" },
    { key: "description", label: "Description", ...longText },
  ];
  if (capability.kind !== "prompt") {
    base.push({
      key: "timeoutMs",
      label: "Timeout (ms)",
      ...optional,
      ...number,
    });
  }
  switch (capability.kind) {
    case "tool":
      base.push(
        {
          key: "inputSchema",
          label: "Input schema (JSON)",
          help: "Use an object-root JSON Schema.",
          ...json,
        },
        {
          key: "outputSchema",
          label: "Output schema (JSON)",
          ...optional,
          ...json,
        },
      );
      break;
    case "resource":
      base.push(
        { key: "uri", label: "Resource URI" },
        { key: "mimeType", label: "Media type" },
      );
      break;
    case "resourceTemplate":
      base.push(
        {
          key: "uriTemplate",
          label: "URI template",
          help: "Use named placeholders such as {item}.",
        },
        { key: "mimeType", label: "Media type" },
        {
          key: "variables",
          label: "Variable schemas (JSON)",
          help: 'Example: {"item":{"type":"string"}}',
          ...json,
        },
      );
      break;
    case "prompt":
      break;
  }
  return fields("/capabilities/" + index, base);
}

export function promptArgumentFields(
  capability: Capability,
  capabilityIndex: number,
): EditorField[][] {
  return capability.kind === "prompt"
    ? capability.arguments.map((_, index) =>
        fields("/capabilities/" + capabilityIndex + "/arguments/" + index, [
          { key: "name", label: "Argument name" },
          { key: "description", label: "Argument description", ...optional },
          { key: "required", label: "Required", format: "toggle" },
        ]),
      )
    : [];
}

export function promptMessageFields(
  capability: Capability,
  capabilityIndex: number,
): EditorField[][] {
  return capability.kind === "prompt"
    ? capability.messages.map((_, index) =>
        fields("/capabilities/" + capabilityIndex + "/messages/" + index, [
          {
            key: "role",
            label: "Message role",
            format: "select",
            choices: ["user", "assistant"],
          },
          {
            key: "textTemplate",
            label: "Message text",
            help: "Use a declared argument in braces, such as {item}.",
            ...longText,
          },
        ]),
      )
    : [];
}

export function bindingFields(binding: Binding, index: number): EditorField[] {
  return fields(
    "/bindings/" + index,
    binding.kind === "rest"
      ? [
          {
            key: "origin",
            label: "Origin",
            help: "Example: https://catalog.example.test",
          },
          {
            key: "method",
            label: "HTTP method",
            format: "select",
            choices: httpMethods,
          },
          {
            key: "pathTemplate",
            label: "Path template",
            help: "Example: /items/{id}",
          },
          {
            key: "pathParams",
            label: "Path parameter pointers (JSON)",
            help: 'Example: {"id":"/id"}',
            ...json,
          },
          { key: "bodyPointer", label: "Body JSON Pointer", ...optional },
          {
            key: "headersFromSecrets",
            label: "Headers from secret references (JSON)",
            help: 'Example: {"Authorization":"catalog_auth"}',
            ...optional,
            ...json,
          },
        ]
      : [
          {
            key: "rootRef",
            label: "File root reference",
            help: "Choose the local folder in Playground.",
          },
          {
            key: "relativePathTemplate",
            label: "Relative path",
            help: "Example: guide.txt or items/{item}.txt",
          },
        ],
  );
}

export function grantFields(grant: Grant, index: number): EditorField[] {
  const prefix = "/policy/grants/" + index;
  const target =
    grant.target.kind === "rest"
      ? [
          { key: "origin", label: "Granted origin" },
          {
            key: "method",
            label: "Granted HTTP method",
            format: "select",
            choices: httpMethods,
          } as const,
          { key: "pathTemplate", label: "Granted path template" },
          {
            key: "allowedPeerRanges",
            label: "Allowed peer ranges (JSON)",
            ...json,
            ...optional,
          },
        ]
      : [
          { key: "rootRef", label: "Granted file root reference" },
          {
            key: "relativePathTemplate",
            label: "Narrower relative path",
            ...optional,
          },
        ];
  return [
    ...fields(
      prefix,
      grant.target.kind === "rest"
        ? [
            {
              key: "operation",
              label: "Operation",
              format: "select",
              choices: ["http.read", "http.write"],
            },
          ]
        : [],
    ),
    ...fields(prefix + "/target", target),
    ...fields(prefix + "/limits", [
      { key: "timeoutMs", label: "Grant timeout (ms)", ...number, ...optional },
      {
        key: "maxOutputBytes",
        label: "Maximum output (bytes)",
        ...number,
        ...optional,
      },
    ]),
  ];
}

export function secretFields(references: SecretRef[]): EditorField[][] {
  return references.map((_, index) =>
    fields("/secretRefs/" + index, [
      { key: "id", label: "Reference ID" },
      { key: "purpose", label: "Purpose" },
    ]),
  );
}

export function scenarioFields(
  scenario: Scenario,
  index: number,
): EditorField[] {
  const prefix = "/scenarios/" + index;
  return [
    ...fields(prefix, [{ key: "name", label: "Scenario name" }]),
    ...fields(
      prefix + "/call",
      scenario.call.method === "resources/read"
        ? [
            {
              key: "uri",
              label: "Resource URI",
              help: "Enter the full URI with template variables filled in.",
            },
          ]
        : [
            {
              key: "name",
              label:
                scenario.call.method === "tools/call"
                  ? "Tool name"
                  : "Prompt name",
            },
            {
              key: "arguments",
              label: "Arguments (JSON)",
              help: 'Example: {"id":"42"}',
              ...json,
            },
          ],
    ),
    ...fields(prefix, [
      {
        key: "fixtures",
        label: "Synthetic fixtures (JSON)",
        ...optional,
        ...json,
        help:
          scenario.call.method === "resources/read"
            ? 'Example: [{"kind":"fileRead","bindingId":"guide","relativePath":"guide.txt","text":"Sample"}]'
            : 'Example: [{"kind":"rest","bindingId":"lookup","method":"GET","path":"/items/42","status":200,"body":{"name":"Sample"}}]',
      },
      {
        key: "assertions",
        label: "Assertions (JSON)",
        ...json,
        help: 'Example: [{"kind":"equals","path":"/structuredContent/name","expected":"Sample"}]',
      },
      {
        key: "sensitivePaths",
        label: "Sensitive result pointers (JSON)",
        ...optional,
        ...json,
        help: 'Example: ["/structuredContent/private"]',
      },
    ]),
  ];
}
