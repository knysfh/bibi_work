import { describe, expect, it, vi } from "vitest";
import { createHttpClient } from "../../../shared/api/http-client";
import type { TokenProvider } from "../../../shared/api/token-provider";
import { createLlmApi } from "./llm.adapter";

const tenantId = "10000000-0000-4000-8000-000000000001";
const providerId = "20000000-0000-4000-8000-000000000001";
const credentialId = "30000000-0000-4000-8000-000000000001";
const profileId = "40000000-0000-4000-8000-000000000001";

const tokenProvider: TokenProvider = {
  getAccessToken: vi.fn(async () => "access-token"),
  setTokenSet: vi.fn(),
  clearTokenSet: vi.fn()
};

describe("llm adapter", () => {
  it("manages providers, credentials, and profiles through public API", async () => {
    const fetchImpl = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = new URL(String(input));
      if (url.pathname === "/api/v1/llm-providers" && init?.method !== "POST") {
        expect(url.searchParams.get("tenant_id")).toBe(tenantId);
        return Response.json([providerDto()]);
      }
      if (url.pathname === "/api/v1/llm-providers" && init?.method === "POST") {
        expect(jsonBody(init)).toMatchObject({
          tenant_id: tenantId,
          provider_key: "openai_compatible",
          display_name: "OpenAI compatible"
        });
        return Response.json(providerDto());
      }
      if (url.pathname === `/api/v1/llm-credentials` && init?.method !== "POST") {
        expect(url.searchParams.get("tenant_id")).toBe(tenantId);
        return Response.json([credentialDto()]);
      }
      if (url.pathname === "/api/v1/llm-credentials" && init?.method === "POST") {
        expect(jsonBody(init)).toMatchObject({
          tenant_id: tenantId,
          provider_id: providerId,
          secret_ref: "env://OPENAI_API_KEY"
        });
        return Response.json(credentialDto());
      }
      if (url.pathname === `/api/v1/llm-credentials/${credentialId}/revoke`) {
        expect(init?.method).toBe("POST");
        return Response.json({ ...credentialDto(), status: "revoked" });
      }
      if (url.pathname === "/api/v1/llm-model-profiles" && init?.method !== "POST") {
        return Response.json([profileDto()]);
      }
      if (url.pathname === "/api/v1/llm-model-profiles" && init?.method === "POST") {
        expect(jsonBody(init)).toMatchObject({
          tenant_id: tenantId,
          provider_id: providerId,
          credential_id: credentialId,
          profile_name: "default",
          model_name: "gpt-test"
        });
        return Response.json(profileDto());
      }
      if (url.pathname === `/api/v1/llm-model-profiles/${profileId}`) {
        expect(init?.method).toBe("PATCH");
        return Response.json(profileDto("active", "gpt-updated"));
      }
      if (url.pathname === `/api/v1/llm-model-profiles/${profileId}/test`) {
        expect(init?.method).toBe("POST");
        expect(jsonBody(init)).toEqual({ tenant_id: tenantId });
        return Response.json({
          success: true,
          provider_key: "openai_compatible",
          model_name: "gpt-test",
          http_status: 200,
          latency_ms: 42,
          message: "LLM provider connection succeeded"
        });
      }
      throw new Error(`unexpected path ${url.pathname}`);
    });
    const api = createLlmApi(
      createHttpClient({
        baseUrl: "http://localhost:8361/api/v1",
        tokenProvider,
        fetchImpl: fetchImpl as typeof fetch
      })
    );

    const providers = await api.listProviders({ tenantId });
    const provider = await api.createProvider({
      tenantId,
      providerKey: "openai_compatible",
      displayName: "OpenAI compatible"
    });
    const credentials = await api.listCredentials({ tenantId });
    const credential = await api.createCredential({
      tenantId,
      providerId,
      secretRef: "env://OPENAI_API_KEY"
    });
    const revoked = await api.revokeCredential({ tenantId, credentialId });
    const profiles = await api.listProfiles({ tenantId });
    const profile = await api.createProfile({
      tenantId,
      providerId,
      credentialId,
      profileName: "default",
      modelName: "gpt-test"
    });
    const updated = await api.updateProfile({
      tenantId,
      profileId,
      modelName: "gpt-updated"
    });
    const testResult = await api.testProfile({ tenantId, profileId });

    expect(providers[0]).toMatchObject({ providerKey: "openai_compatible" });
    expect(provider.displayName).toBe("OpenAI compatible");
    expect(credentials[0]).toMatchObject({ hasSecretRef: true });
    expect(credentials[0].raw.name).not.toContain("env://");
    expect(credential.providerId).toBe(providerId);
    expect(revoked.status).toBe("revoked");
    expect(profiles[0]).toMatchObject({ credentialId, modelName: "gpt-test" });
    expect(profile.profileName).toBe("default");
    expect(updated.modelName).toBe("gpt-updated");
    expect(testResult).toMatchObject({ success: true, httpStatus: 200, latencyMs: 42 });
  });
});

function providerDto() {
  return {
    id: providerId,
    tenant_id: tenantId,
    name: "OpenAI compatible",
    description: "openai_compatible",
    status: "active",
    metadata: {
      provider_key: "openai_compatible",
      base_url: "https://example.test/v1",
      auth_scheme: "bearer",
      default_headers_template: {}
    },
    created_at: "2026-06-22T00:00:00Z",
    updated_at: "2026-06-22T00:00:00Z"
  };
}

function credentialDto() {
  return {
    id: credentialId,
    tenant_id: tenantId,
    name: "credential 30000000",
    description: "OpenAI compatible",
    status: "active",
    metadata: {
      provider_id: providerId,
      provider_key: "openai_compatible",
      provider_name: "OpenAI compatible",
      owner_scope: "tenant",
      owner_resource_id: null,
      has_secret_ref: true,
      has_secret_hash: false,
      expires_at: null,
      revoked_at: null
    },
    created_at: "2026-06-22T00:00:00Z",
    updated_at: null
  };
}

function profileDto(status = "active", modelName = "gpt-test") {
  return {
    id: profileId,
    tenant_id: tenantId,
    name: "default",
    description: modelName,
    status,
    metadata: {
      provider_id: providerId,
      credential_id: credentialId,
      context_window: 128000,
      max_input_tokens: 64000,
      max_output_tokens: 4096,
      temperature: 0.2,
      top_p: 1,
      reasoning_effort: null,
      response_format: {},
      tool_choice_policy: {},
      rate_limit_policy: {},
      cost_policy: {}
    },
    created_at: "2026-06-22T00:00:00Z",
    updated_at: "2026-06-22T00:00:00Z"
  };
}

function jsonBody(init?: RequestInit) {
  return JSON.parse(String(init?.body ?? "{}")) as unknown;
}
