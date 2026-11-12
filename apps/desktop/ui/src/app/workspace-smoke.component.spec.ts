import { TestBed } from "@angular/core/testing";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { afterEach, describe, expect, expectTypeOf, it, vi } from "vitest";
import smoke from "../../../../../crates/builder-app/tests/fixtures/workspace-smoke.json";
import type { Diagnostic, Reply, SmokeRequest, SmokeStatus } from "./smoke-ipc";
import { WorkspaceSmokeComponent } from "./workspace-smoke.component";

describe("workspace smoke IPC", () => {
  afterEach(() => clearMocks());

  it("sends the typed request and displays the Rust success reply", async () => {
    const { request, reply } = smoke.supported;
    expectTypeOf(request).toEqualTypeOf<SmokeRequest>();
    expectTypeOf(reply.data).toEqualTypeOf<SmokeStatus>();
    const typedReply = { ...reply, ok: true } satisfies Reply<SmokeStatus>;
    expect(typedReply).toEqual(reply);

    const invoke = vi.fn(() => typedReply);
    mockIPC(invoke);
    const fixture = TestBed.createComponent(WorkspaceSmokeComponent);
    await fixture.whenStable();
    const element: HTMLElement = fixture.nativeElement;
    element.querySelector("button")!.click();
    await fixture.whenStable();

    expect(invoke).toHaveBeenCalledExactlyOnceWith("workspace_smoke", {
      request,
    });
    const status = element.querySelector('[role="status"]')?.textContent;
    expect(status).toContain("Workspace check passed.");
    expect(status).toContain(reply.data.builderVersion);
    expect(status).toContain(reply.data.correlationId);
  });

  it("displays the Rust diagnostic if the native contract differs", async () => {
    const { reply } = smoke.unsupported;
    expectTypeOf(reply.error.details).toEqualTypeOf<
      NonNullable<Diagnostic["details"]>
    >();
    const typedReply = {
      ...reply,
      ok: false,
      error: { ...reply.error, code: "input.invalid", severity: "error" },
    } satisfies Reply<SmokeStatus>;
    expect(typedReply).toEqual(reply);

    mockIPC(() => typedReply);
    const fixture = TestBed.createComponent(WorkspaceSmokeComponent);
    await fixture.whenStable();
    const element: HTMLElement = fixture.nativeElement;
    element.querySelector("button")!.click();
    await fixture.whenStable();

    const status = element.querySelector('[role="status"]')?.textContent;
    expect(status).toContain(reply.error.message);
    expect(status).toContain(reply.error.action);
  });
});
