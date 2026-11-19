import { invoke } from "@tauri-apps/api/core";
import type {
  Grant,
  Invocation,
  Json,
  JsonObject,
  ProjectDocument,
  ProjectSnapshot,
  ProjectSummary,
  Revision,
} from "./project-model";

export interface Diagnostic {
  code: string;
  message: string;
  severity: "error" | "warning";
  correlationId: string;
  action?: string;
  field?: string;
  capabilityId?: string;
  details?: unknown;
}

export type Reply<T> =
  | { ok: true; data: T; diagnostics: Diagnostic[] }
  | { ok: false; error: Diagnostic; diagnostics: Diagnostic[] };

interface SelectionHandle {
  selectionId: string;
}

export interface PathSelection extends SelectionHandle {
  displayPath: string;
}

export type PathPurpose =
  | "projectRoot"
  | "projectDestination"
  | "recoveryDestination"
  | "exportParent"
  | "previousExport"
  | "fileRoot"
  | "clientExecutable"
  | "clientManifest"
  | "clientRuntimeConfig";

export function handle(selection: PathSelection): SelectionHandle {
  return { selectionId: selection.selectionId };
}

export type ProjectRoot = SelectionHandle | { registrationId: string };

export interface SavedProject {
  registrationId: string;
  expectedRevision: Revision;
}

export interface RecoveryCandidate {
  path: string;
  revision: Revision;
  kind: "lastValid" | "interruptedWrite";
}

export interface RecoveryInfo {
  currentRevision: Revision | null;
  candidates: RecoveryCandidate[];
}

export interface ExportConflict {
  path: string;
  previousDigest: Revision | null;
  observedDigest: Revision | null;
  replacementDigest: Revision | null;
  canKeep: boolean;
}

interface GenerationRecord {
  version: number;
  projectId: string;
  projectSchemaVersion: number;
  snapshotDigest: Revision;
  generatorVersion: string;
  runtimeVersion: string;
  templateVersion: string;
  protocolVersions: string[];
  cargoLockDigest: Revision;
  files: { path: string; owner: "generator" | "project"; digest: Revision }[];
}

export interface GenerationPlan {
  planId: string;
  planDigest: Revision;
  destinationDisplayPath: string;
  generation: GenerationRecord | null;
  conflicts: ExportConflict[];
}

export interface GenerationReceipt {
  destinationDisplayPath: string;
  generation: GenerationRecord;
}

export type ClientConfigTarget =
  "claude-code" | "codex" | "opencode" | "generic";

interface ClientConfigExport {
  target: ClientConfigTarget;
  suggestedFileName: string;
  contents: string;
  destination?: string;
}

export interface ClientConfigPreview extends ClientConfigExport {
  configurationId: string;
}

export type ConflictDecision = "keep" | "regenerate";

export interface ConflictResolution {
  path: string;
  observedDigest: Revision | null;
  replacementDigest: Revision | null;
  decision: ConflictDecision;
}

export type ProtocolRevision = "2026-07-28" | "2025-11-25";
export type ProtocolPreference = "auto" | ProtocolRevision;
export type PreviewMode =
  | { kind: "fixture"; scenarioId: string }
  | { kind: "live"; scenarioId?: string };
type CredentialInput =
  { kind: "session"; value: string } | { kind: "environment"; name: string };
type CredentialReference =
  { kind: "session" } | { kind: "environment"; name: string };

export interface LaunchSubject extends SavedProject {
  kind: "launch";
}

export interface PreviewAccess {
  approvedGrantIds: string[];
  approvedPromptIds: string[];
  fileRoots?: Record<string, SelectionHandle>;
  credentials?: Record<string, CredentialInput>;
}

export interface LaunchReview {
  reviewId: string;
  digest: Revision;
  summary: {
    digest: Revision;
    registrationId: string;
    projectId: string;
    manifestRevision: Revision;
    mode: PreviewMode;
    executable: string;
    arguments: string[];
    workingDirectory: string;
    environment: Record<string, string>;
    approvedGrants: Grant[];
    approvedPromptIds: string[];
    fileRoots: Record<string, string>;
    credentialSources: Record<string, CredentialReference>;
    credentialTargets: {
      capabilityId: string;
      bindingId: string;
      origin: string;
      references: string[];
    }[];
    limits: { timeoutMs?: number; maxOutputBytes?: number };
  };
}

export interface Discovery {
  protocolRevision: ProtocolRevision;
  serverInfo?: JsonObject;
  capabilities: { tools: boolean; resources: boolean; prompts: boolean };
  supportedMethods: string[];
  tools: { name: string; description?: string; inputSchema: Json }[];
  resources: { name: string; uri: string; description?: string }[];
  resourceTemplates: {
    name: string;
    uriTemplate: string;
    description?: string;
  }[];
  prompts: {
    name: string;
    description?: string;
    arguments?: { name: string; required?: boolean }[];
  }[];
}

type RunOutcome =
  | "passed"
  | "assertionFailed"
  | "invalid"
  | "denied"
  | "failed"
  | "timedOut"
  | "cancelled";

interface TraceSummary {
  entries: number;
  droppedEntries: number;
  truncated: boolean;
}

export interface TraceSnapshot {
  summary: TraceSummary;
  entries: {
    sequence: number;
    offsetMs: number;
    direction: "sent" | "received" | "stderr";
    kind: string;
    method?: string;
    messageId?: number;
    correlationId: string;
    payload?: Json;
    truncated: boolean;
  }[];
}

export interface PreviewRun {
  result: {
    version: 1;
    runId: string;
    correlationId: string;
    scenarioId?: string;
    target: "managedFixture" | "managedLive";
    transport: "stdio";
    protocolRevision: ProtocolRevision;
    startedAt: string;
    durationMs: number;
    outcome: RunOutcome;
    effect: string;
    assertions: {
      index: number;
      path?: string;
      passed: boolean;
      code?: string;
    }[];
    diagnostic?: Pick<Diagnostic, "code" | "message" | "correlationId">;
    traceSummary: TraceSummary;
  };
  payload?: Json;
}

export const outcomeNames: Record<RunOutcome, string> = {
  passed: "Passed",
  assertionFailed: "Assertion failed",
  invalid: "Invalid input",
  denied: "Permission denied",
  failed: "Failed",
  timedOut: "Timed out",
  cancelled: "Cancelled",
};

export type RunEvent =
  | {
      cursor: number;
      kind: "started";
      connectionId: string;
      discovery: Discovery;
    }
  | { cursor: number; kind: "completed"; run: PreviewRun }
  | { cursor: number; kind: "failed"; error: Diagnostic };

interface Command<Request, Data> {
  request: Request;
  data: Data;
}

type Commands = {
  select_path: Command<{ purpose: PathPurpose }, PathSelection | null>;
  project_list: Command<Record<string, never>, ProjectSummary[]>;
  project_open: Command<{ root: ProjectRoot }, ProjectSnapshot>;
  project_create: Command<
    { name: string; destination: SelectionHandle },
    ProjectSnapshot
  >;
  project_validate: Command<{ document: ProjectDocument }, { valid: true }>;
  project_save: Command<
    SavedProject & { document: ProjectDocument },
    ProjectSnapshot
  >;
  project_recover: Command<
    (
      | { registrationId: string; root?: never }
      | { registrationId?: never; root: SelectionHandle }
    ) & {
      expectedRevision?: Revision | null;
      source:
        | { kind: "backup"; revision: Revision }
        | { kind: "draft"; document: ProjectDocument };
      destination?: SelectionHandle;
    },
    ProjectSnapshot
  >;
  generation_plan: Command<
    SavedProject & {
      destination: SelectionHandle;
      folderName: string;
      fromExport?: SelectionHandle;
    },
    GenerationPlan
  >;
  generation_write: Command<
    {
      planId: string;
      planDigest: Revision;
      resolutions?: ConflictResolution[];
    },
    GenerationReceipt
  >;
  client_config_export: Command<
    {
      target: ClientConfigTarget;
      name: string;
      executable: SelectionHandle;
      manifest: SelectionHandle;
      runtimeConfig?: SelectionHandle;
    },
    ClientConfigPreview
  >;
  client_config_save: Command<
    { configurationId: string },
    ClientConfigExport | null
  >;
  policy_review: Command<
    {
      subject: LaunchSubject;
      mode: PreviewMode;
      access: PreviewAccess;
      protocol: ProtocolPreference;
      capturePayloads: boolean;
    },
    LaunchReview
  >;
  policy_approve: Command<
    { reviewId: string; digest: Revision },
    { approvalId: string; expiresAt: string }
  >;
  playground_start: Command<
    { subject: LaunchSubject; approval: { approvalId: string } },
    { operationId: string }
  >;
  run_poll: Command<
    { id: string; afterCursor?: number },
    { events: RunEvent[]; nextCursor: number; terminal: boolean }
  >;
  run_cancel: Command<{ id: string }, { cancellationRequested: boolean }>;
  playground_discover: Command<{ connectionId: string }, Discovery>;
  playground_call: Command<
    { connectionId: string; call: Invocation; timeoutMs?: number },
    { runId: string }
  >;
  scenario_run: Command<
    SavedProject & {
      scenarioId: string;
      connectionId: string;
      timeoutMs?: number;
    },
    { runId: string }
  >;
  playground_trace: Command<{ connectionId: string }, TraceSnapshot>;
  playground_stop: Command<{ connectionId: string }, { stopped: true }>;
};

export async function desktop<K extends keyof Commands>(
  command: K,
  request: Commands[K]["request"],
): Promise<Reply<Commands[K]["data"]>> {
  try {
    return await invoke<Reply<Commands[K]["data"]>>(command, { request });
  } catch {
    // Invocation errors can contain native exception text or the rejected request.
    return {
      ok: false,
      error: {
        code: "desktop.ipc_unavailable",
        message: "The desktop service could not complete the request.",
        severity: "error",
        action:
          "Try again. If the problem continues, restart MCP Server Builder.",
        correlationId: "",
      },
      diagnostics: [],
    };
  }
}

export function diagnosticsFrom<T>(reply: Reply<T>): Diagnostic[] {
  return reply.ok ? reply.diagnostics : [reply.error, ...reply.diagnostics];
}
