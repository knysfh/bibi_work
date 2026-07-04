import { describe, expect, it, vi } from "vitest";
import { createHttpClient } from "../../../shared/api/http-client";
import type { TokenProvider } from "../../../shared/api/token-provider";
import { createConversationApi } from "./conversation.adapter";

const tokenProvider: TokenProvider = {
  getAccessToken: vi.fn(async () => "access-token"),
  setTokenSet: vi.fn(),
  clearTokenSet: vi.fn()
};

describe("conversation adapter", () => {
  it("lists conversations using tenant query and maps responses", async () => {
    const fetchImpl = vi.fn(async (input: RequestInfo | URL) => {
      const url = new URL(String(input));
      expect(url.pathname).toBe("/api/v1/conversations");
      expect(url.searchParams.get("tenant_id")).toBe("10000000-0000-4000-8000-000000000001");
      return Response.json([
        {
          id: "20000000-0000-4000-8000-000000000001",
          tenant_id: "10000000-0000-4000-8000-000000000001",
          project_id: null,
          agent_id: null,
          title: "Daily run",
          status: "active",
          metadata: {},
          created_at: "2026-06-21T00:00:00Z",
          updated_at: "2026-06-21T00:00:00Z"
        }
      ]);
    });
    const conversationApi = createConversationApi(
      createHttpClient({
        baseUrl: "http://localhost:8361/api/v1",
        tokenProvider,
        fetchImpl: fetchImpl as typeof fetch
      })
    );

    const conversations = await conversationApi.listConversations(
      "10000000-0000-4000-8000-000000000001"
    );

    expect(conversations).toHaveLength(1);
    expect(conversations[0]).toMatchObject({ title: "Daily run", projectId: undefined });
  });
});
