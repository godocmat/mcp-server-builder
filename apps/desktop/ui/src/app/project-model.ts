export type Json =
  null | boolean | number | string | Json[] | { [key: string]: Json };
export type JsonObject = { [key: string]: Json };

export type Revision = string;

export interface ProjectDocument {
  format: "mcp-server-builder.project";
  schemaVersion: 1;
  projectId: string;
  name: string;
  server: {
    name: string;
    version: string;
    description?: string;
    instructions?: string;
  };
  transport: "stdio";
  capabilities: Capability[];
  bindings: Binding[];
  policy: { grants: Grant[] };
  secretRefs: SecretRef[];
  scenarios: Scenario[];
}

export interface ProjectSnapshot {
  registrationId: string;
  rootDisplayPath: string;
  revision: Revision;
  document: ProjectDocument;
}

export interface ProjectSummary {
  registrationId: string;
  rootDisplayPath: string;
  name: string;
  lastKnownRevision: Revision;
  status: "available" | "changed" | "missing" | "unavailable";
}

interface CapabilityBase {
  id: string;
  name: string;
  description: string;
}

interface BoundCapability extends CapabilityBase {
  bindingId: string;
  timeoutMs?: number;
}

export type Capability =
  | (BoundCapability & { kind: "tool"; inputSchema: Json; outputSchema?: Json })
  | (BoundCapability & { kind: "resource"; uri: string; mimeType: string })
  | (BoundCapability & {
      kind: "resourceTemplate";
      uriTemplate: string;
      variables: Record<string, Json>;
      mimeType: string;
    })
  | (CapabilityBase & {
      kind: "prompt";
      arguments: { name: string; description?: string; required: boolean }[];
      messages: { role: "user" | "assistant"; textTemplate: string }[];
    });

export type RestBinding = {
  kind: "rest";
  origin: string;
  method: string;
  pathTemplate: string;
  pathParams: Record<string, string>;
  bodyPointer?: string;
  headersFromSecrets?: Record<string, string>;
};

export type FileBinding = {
  kind: "fileRead";
  rootRef: string;
  relativePathTemplate: string;
};

export type Binding = (RestBinding | FileBinding) & { id: string };

export type GrantTarget =
  | (Pick<RestBinding, "kind" | "origin" | "method" | "pathTemplate"> & {
      allowedPeerRanges?: string[];
    })
  | { kind: "fileRead"; rootRef: string; relativePathTemplate?: string };

export interface Grant {
  id: string;
  capabilityId: string;
  operation: "http.read" | "http.write" | "file.read";
  target: GrantTarget;
  limits?: { timeoutMs?: number; maxOutputBytes?: number };
}

export interface SecretRef {
  id: string;
  purpose: string;
}

export type Invocation =
  | { method: "tools/call"; name: string; arguments: JsonObject }
  | { method: "resources/read"; uri: string }
  | { method: "prompts/get"; name: string; arguments: Record<string, string> };

export type Fixture =
  | {
      kind: "rest";
      bindingId: string;
      method: string;
      path: string;
      status: number;
      body: Json;
    }
  | { kind: "fileRead"; bindingId: string; relativePath: string; text: string };

export type Assertion =
  | { kind: "equals"; path: string; expected: Json }
  | { kind: "schema"; path: string; schema: Json }
  | { kind: "errorCode"; code: string };

export interface Scenario {
  version: 1;
  id: string;
  name: string;
  mode: "fixture" | "live";
  call: Invocation;
  fixtures?: Fixture[];
  assertions: Assertion[];
  sensitivePaths?: string[];
}

export const capabilityLabels = {
  tool: "REST tool",
  resource: "File resource",
  resourceTemplate: "Resource template",
  prompt: "Prompt",
} satisfies Record<Capability["kind"], string>;

function uniqueId(prefix: string, items: { id: string }[]): string {
  const taken = new Set(items.map((item) => item.id));
  let candidate = prefix;
  for (let suffix = 2; taken.has(candidate); suffix++) {
    candidate = prefix + "_" + suffix;
  }
  return candidate;
}

/** Creates an editable starting point. The shared service validates before writing. */
export function addCapability(
  document: ProjectDocument,
  kind: Capability["kind"],
): ProjectDocument {
  const prefix = {
    tool: "rest_tool",
    resource: "file_resource",
    resourceTemplate: "file_template",
    prompt: "prompt",
  };
  const id = uniqueId(prefix[kind], document.capabilities);
  const base = { id, name: id, description: capabilityLabels[kind] };
  const bindingId = uniqueId(id + "_binding", document.bindings);
  let capability: Capability;
  let binding: Binding | undefined;
  switch (kind) {
    case "tool":
      capability = {
        ...base,
        kind,
        bindingId,
        inputSchema: { type: "object", properties: {} },
      };
      binding = {
        id: bindingId,
        kind: "rest",
        origin: "",
        method: "GET",
        pathTemplate: "/",
        pathParams: {},
      };
      break;
    case "resource":
      capability = {
        ...base,
        kind,
        bindingId,
        uri: "file://local/" + id,
        mimeType: "text/plain",
      };
      binding = {
        id: bindingId,
        kind: "fileRead",
        rootRef: "docs",
        relativePathTemplate: "guide.txt",
      };
      break;
    case "resourceTemplate":
      capability = {
        ...base,
        kind,
        bindingId,
        uriTemplate: "file://local/" + id + "/{item}",
        variables: { item: { type: "string" } },
        mimeType: "text/plain",
      };
      binding = {
        id: bindingId,
        kind: "fileRead",
        rootRef: "docs",
        relativePathTemplate: "{item}.txt",
      };
      break;
    case "prompt":
      capability = {
        ...base,
        kind,
        arguments: [],
        messages: [{ role: "user", textTemplate: "" }],
      };
      break;
  }
  return {
    ...document,
    capabilities: [...document.capabilities, capability],
    bindings: binding ? [...document.bindings, binding] : document.bindings,
  };
}

export function addGrant(
  document: ProjectDocument,
  capability: BoundCapability,
  binding: Binding,
): ProjectDocument {
  const id = uniqueId("grant", document.policy.grants);
  const isRead =
    binding.kind === "rest" &&
    ["GET", "HEAD", "OPTIONS"].includes(binding.method);
  const grant: Grant =
    binding.kind === "rest"
      ? {
          id,
          capabilityId: capability.id,
          operation: isRead ? "http.read" : "http.write",
          target: {
            kind: "rest",
            origin: binding.origin,
            method: binding.method,
            pathTemplate: binding.pathTemplate,
          },
        }
      : {
          id,
          capabilityId: capability.id,
          operation: "file.read",
          target: {
            kind: "fileRead",
            rootRef: binding.rootRef,
            relativePathTemplate: binding.relativePathTemplate,
          },
        };
  return {
    ...document,
    policy: { grants: [...document.policy.grants, grant] },
  };
}

export function addScenario(document: ProjectDocument): ProjectDocument {
  const id = uniqueId("scenario", document.scenarios);
  const first = document.capabilities[0];
  const call: Invocation =
    !first || first.kind === "tool"
      ? { method: "tools/call", name: first?.name ?? "", arguments: {} }
      : first.kind === "prompt"
        ? { method: "prompts/get", name: first.name, arguments: {} }
        : {
            method: "resources/read",
            uri: first.kind === "resource" ? first.uri : first.uriTemplate,
          };
  const scenario: Scenario = {
    version: 1,
    id,
    name: "Fixture scenario",
    mode: "fixture",
    call,
    fixtures: [],
    assertions: [{ kind: "schema", path: "", schema: { type: "object" } }],
  };
  return { ...document, scenarios: [...document.scenarios, scenario] };
}
