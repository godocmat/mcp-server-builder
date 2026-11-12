import { invoke } from "@tauri-apps/api/core";

export const SMOKE_CONTRACT_VERSION = 1;

export interface Diagnostic {
  code: "cli.usage" | "input.invalid";
  message: string;
  severity: "error" | "warning";
  correlationId: string;
  action?: string;
  field?: string;
  capabilityId?: string;
  details?: { supportedVersions: number[] };
}

export type Reply<T> =
  | { ok: true; data: T; diagnostics: Diagnostic[] }
  | { ok: false; error: Diagnostic; diagnostics: Diagnostic[] };

export interface SmokeRequest {
  contractVersion: number;
}

export interface SmokeStatus {
  builderVersion: string;
  contractVersion: number;
  correlationId: string;
}

export function checkWorkspace(): Promise<Reply<SmokeStatus>> {
  const request: SmokeRequest = { contractVersion: SMOKE_CONTRACT_VERSION };
  return invoke<Reply<SmokeStatus>>("workspace_smoke", { request });
}
