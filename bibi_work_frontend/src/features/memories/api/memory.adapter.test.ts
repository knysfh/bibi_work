import { describe, expect, it, vi } from "vitest";
import { createHttpClient } from "../../../shared/api/http-client";
import type { TokenProvider } from "../../../shared/api/token-provider";
import { createMemoryApi } from "./memory.adapter";

const tokenProvider: TokenProvider = {
  getAccessToken: vi.fn(async () => "access-token"),
  setTokenSet: vi.fn(),
  clearTokenSet: vi.fn()
};

describe("memory adapter", () => {
  it("lists memories with governance filters and maps source run", async () => {
    const fetchImpl = vi.fn(async (input: RequestInfo | URL) => {
      const url = new URL(String(input));
      expect(url.pathname).toBe("/api/v1/memories");
      expect(url.searchParams.get("tenant_id")).toBe("10000000-0000-4000-8000-000000000001");
      expect(url.searchParams.get("status")).toBe("candidate");
      expect(url.searchParams.get("layer")).toBe("semantic");
      return Response.json([memoryDto({ source_run_id: "50000000-0000-4000-8000-000000000001" })]);
    });
    const memoryApi = createMemoryApi(
      createHttpClient({
        baseUrl: "http://localhost:8361/api/v1",
        tokenProvider,
        fetchImpl: fetchImpl as typeof fetch
      })
    );

    const memories = await memoryApi.listMemories({
      tenantId: "10000000-0000-4000-8000-000000000001",
      status: "candidate",
      layer: "semantic"
    });

    expect(memories[0]).toMatchObject({
      layer: "semantic",
      status: "candidate",
      sourceRunId: "50000000-0000-4000-8000-000000000001"
    });
  });

  it("sends batch decision payloads and maps item failures", async () => {
    const fetchImpl = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      expect(new URL(String(input)).pathname).toBe("/api/v1/memories:batch-decision");
      expect(JSON.parse(String(init?.body))).toMatchObject({
        tenant_id: "10000000-0000-4000-8000-000000000001",
        decision: "activate",
        memory_ids: ["20000000-0000-4000-8000-000000000001"]
      });
      return Response.json({
        decision: "activate",
        target_status: "approved",
        succeeded: 0,
        failed: 1,
        results: [
          {
            memory_id: "20000000-0000-4000-8000-000000000001",
            status: "failed",
            memory: null,
            error_code: "FORBIDDEN",
            error_message: "denied"
          }
        ]
      });
    });
    const memoryApi = createMemoryApi(
      createHttpClient({
        baseUrl: "http://localhost:8361/api/v1",
        tokenProvider,
        fetchImpl: fetchImpl as typeof fetch
      })
    );

    const response = await memoryApi.batchDecideMemories({
      tenantId: "10000000-0000-4000-8000-000000000001",
      decision: "activate",
      memoryIds: ["20000000-0000-4000-8000-000000000001"]
    });

    expect(response).toMatchObject({
      targetStatus: "approved",
      failed: 1,
      results: [{ errorCode: "FORBIDDEN" }]
    });
  });
});

function memoryDto(overrides: Record<string, unknown> = {}) {
  return {
    id: "20000000-0000-4000-8000-000000000001",
    tenant_id: "10000000-0000-4000-8000-000000000001",
    user_id: "30000000-0000-4000-8000-000000000001",
    agent_id: null,
    project_id: null,
    source_run_id: null,
    layer: "semantic",
    content: "Remember staged rollout rules",
    confidence: 0.8,
    status: "candidate",
    visibility: "private",
    sensitivity: "normal",
    created_at: "2026-06-22T00:00:00Z",
    updated_at: "2026-06-22T00:00:00Z",
    ...overrides
  };
}
