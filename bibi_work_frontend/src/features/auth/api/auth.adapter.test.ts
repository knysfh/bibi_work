import { describe, expect, it, vi } from "vitest";
import { createHttpClient } from "../../../shared/api/http-client";
import type { TokenProvider } from "../../../shared/api/token-provider";
import { createAuthApi } from "./auth.adapter";

const tokenProvider: TokenProvider = {
  getAccessToken: vi.fn(async () => "access-token"),
  setTokenSet: vi.fn(),
  clearTokenSet: vi.fn()
};

describe("auth adapter", () => {
  it("loads /me through the shared HTTP client and maps snake_case fields", async () => {
    const fetchImpl = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      expect(String(input)).toBe("http://localhost:8361/api/v1/me");
      expect(init?.headers).toMatchObject({ Authorization: "Bearer access-token" });
      return Response.json({
        tenant_id: "10000000-0000-4000-8000-000000000001",
        user: {
          id: "20000000-0000-4000-8000-000000000001",
          tenant_id: "10000000-0000-4000-8000-000000000001",
          ferriskey_subject: "sub",
          username: "alon",
          email: "alon@example.com",
          display_name: "Alon",
          status: "active",
          created_at: "2026-06-21T00:00:00Z",
          updated_at: "2026-06-21T00:00:00Z"
        },
        tenants: [
          {
            id: "10000000-0000-4000-8000-000000000001",
            name: "Bibi Work",
            slug: "bibi-work",
            membership_role: "admin",
            metadata: {}
          }
        ],
        roles: ["platform_admin"],
        capabilities: ["conversation:create"],
        device: {
          id: "30000000-0000-4000-8000-000000000001",
          tenant_id: "10000000-0000-4000-8000-000000000001",
          device_name: "desktop",
          platform: "linux",
          trust_level: "standard",
          last_seen_at: null,
          revoked_at: null
        },
        session: {
          id: "40000000-0000-4000-8000-000000000001",
          tenant_id: "10000000-0000-4000-8000-000000000001",
          device_id: "30000000-0000-4000-8000-000000000001",
          token_exp: "2026-06-21T01:00:00Z",
          last_seen_at: null,
          source_ip: null,
          user_agent: null,
          revoked_at: null
        }
      });
    });
    const authApi = createAuthApi(
      createHttpClient({
        baseUrl: "http://localhost:8361/api/v1",
        tokenProvider,
        fetchImpl: fetchImpl as typeof fetch
      })
    );

    const me = await authApi.getMe();

    expect(me.tenantId).toBe("10000000-0000-4000-8000-000000000001");
    expect(me.user.displayName).toBe("Alon");
    expect(me.device.deviceName).toBe("desktop");
  });
});
