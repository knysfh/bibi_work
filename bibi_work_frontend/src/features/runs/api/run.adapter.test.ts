import { describe, expect, it, vi } from "vitest";
import { createHttpClient } from "../../../shared/api/http-client";
import type { TokenProvider } from "../../../shared/api/token-provider";
import { createRunApi } from "./run.adapter";

const tokenProvider: TokenProvider = {
  getAccessToken: vi.fn(async () => "access-token"),
  setTokenSet: vi.fn(),
  clearTokenSet: vi.fn()
};

describe("run adapter", () => {
  it("passes after_seq when creating a run stream", async () => {
    const fetchImpl = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = new URL(String(input));
      expect(url.pathname).toBe(
        "/api/v1/conversations/20000000-0000-4000-8000-000000000001/runs:stream"
      );
      expect(url.searchParams.get("after_seq")).toBe("12");
      expect(JSON.parse(String(init?.body))).toMatchObject({
        tenant_id: "10000000-0000-4000-8000-000000000001",
        input: { messages: [{ role: "user", content: "hello" }] }
      });
      return new Response(sseEventDto(), {
        headers: { "Content-Type": "text/event-stream" }
      });
    });
    const runApi = createRunApi(
      createHttpClient({
        baseUrl: "http://localhost:8361/api/v1",
        tokenProvider,
        fetchImpl: fetchImpl as typeof fetch
      })
    );
    const events: string[] = [];

    await runApi.createRunStream(
      {
        tenantId: "10000000-0000-4000-8000-000000000001",
        conversationId: "20000000-0000-4000-8000-000000000001",
        input: { messages: [{ role: "user", content: "hello" }] },
        afterSeq: 12
      },
      (event) => events.push(event.type)
    );

    expect(events).toEqual(["run.started"]);
  });
});

function sseEventDto() {
  return `id: 13
event: run.started
data: {"id":"40000000-0000-4000-8000-000000000001","tenant_id":"10000000-0000-4000-8000-000000000001","conversation_id":"20000000-0000-4000-8000-000000000001","run_id":"30000000-0000-4000-8000-000000000001","seq":13,"event_id":"run.started.1","type":"run.started","payload":{"status":"running"},"trace_id":"trace","created_at":"2026-06-22T00:00:00Z"}

`;
}
