import { describe, expect, it, vi } from "vitest";
import { createHttpClient } from "../../../shared/api/http-client";
import type { TokenProvider } from "../../../shared/api/token-provider";
import { createCatalogApi } from "./catalog.adapter";

const tokenProvider: TokenProvider = {
  getAccessToken: vi.fn(async () => "access-token"),
  setTokenSet: vi.fn(),
  clearTokenSet: vi.fn()
};

describe("catalog adapter", () => {
  it("lists catalog resources and versions with tenant query parameters", async () => {
    const fetchImpl = vi.fn(async (input: RequestInfo | URL) => {
      const url = new URL(String(input));
      expect(url.searchParams.get("tenant_id")).toBe("10000000-0000-4000-8000-000000000001");
      if (url.pathname === "/api/v1/agents") {
        expect(url.searchParams.get("status")).toBe("active");
        return Response.json([resourceDto("20000000-0000-4000-8000-000000000001", "planner")]);
      }
      if (url.pathname === "/api/v1/agents/20000000-0000-4000-8000-000000000001/versions") {
        return Response.json([
          {
            id: "30000000-0000-4000-8000-000000000001",
            tenant_id: "10000000-0000-4000-8000-000000000001",
            parent_id: "20000000-0000-4000-8000-000000000001",
            version_label: "v1",
            snapshot: { model: "gpt" },
            policy_version: "local-v1",
            status: "published",
            created_at: "2026-06-22T00:00:00Z"
          }
        ]);
      }
      throw new Error(`unexpected path ${url.pathname}`);
    });
    const catalogApi = createCatalogApi(
      createHttpClient({
        baseUrl: "http://localhost:8361/api/v1",
        tokenProvider,
        fetchImpl: fetchImpl as typeof fetch
      })
    );

    const agents = await catalogApi.listResources({
      tenantId: "10000000-0000-4000-8000-000000000001",
      kind: "agents",
      status: "active"
    });
    const versions = await catalogApi.listVersions({
      tenantId: "10000000-0000-4000-8000-000000000001",
      kind: "agents",
      resourceId: agents[0].id
    });

    expect(agents[0]).toMatchObject({ name: "planner", status: "active" });
    expect(versions[0]).toMatchObject({ versionLabel: "v1", policyVersion: "local-v1" });
  });

  it("lists mcp tools and policy bindings without exposing secret values", async () => {
    const fetchImpl = vi.fn(async (input: RequestInfo | URL) => {
      const url = new URL(String(input));
      if (url.pathname === "/api/v1/mcp-servers/20000000-0000-4000-8000-000000000002/tools") {
        return Response.json([resourceDto("40000000-0000-4000-8000-000000000001", "lookup")]);
      }
      if (url.pathname === "/api/v1/policy-bindings") {
        expect(url.searchParams.get("resource_type")).toBe("mcp_server");
        expect(url.searchParams.get("resource_id")).toBe("20000000-0000-4000-8000-000000000002");
        expect(url.searchParams.get("include_disabled")).toBe("true");
        return Response.json([
          {
            id: "50000000-0000-4000-8000-000000000001",
            tenant_id: "10000000-0000-4000-8000-000000000001",
            resource_type: "mcp_server",
            resource_id: "20000000-0000-4000-8000-000000000002",
            action: "execute",
            subject_type: "role",
            subject_id: "mcp_user",
            effect: "review",
            risk_level: "high",
            obligations: { audit_level: "high" },
            policy_version: "local-v1",
            created_by_user_id: null,
            created_at: "2026-06-22T00:00:00Z",
            disabled_at: null
          }
        ]);
      }
      throw new Error(`unexpected path ${url.pathname}`);
    });
    const catalogApi = createCatalogApi(
      createHttpClient({
        baseUrl: "http://localhost:8361/api/v1",
        tokenProvider,
        fetchImpl: fetchImpl as typeof fetch
      })
    );

    const tools = await catalogApi.listMcpTools({
      tenantId: "10000000-0000-4000-8000-000000000001",
      mcpServerId: "20000000-0000-4000-8000-000000000002"
    });
    const policies = await catalogApi.listPolicyBindings({
      tenantId: "10000000-0000-4000-8000-000000000001",
      resourceType: "mcp_server",
      resourceId: "20000000-0000-4000-8000-000000000002",
      includeDisabled: true
    });

    expect(tools[0].name).toBe("lookup");
    expect(policies[0]).toMatchObject({ effect: "review", riskLevel: "high" });
  });

  it("creates, updates, and discovers MCP servers through public API", async () => {
    const fetchImpl = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = new URL(String(input));
      if (url.pathname === "/api/v1/mcp-servers") {
        expect(init?.method).toBe("POST");
        expect(jsonBody(init)).toMatchObject({
          tenant_id: "10000000-0000-4000-8000-000000000001",
          name: "filesystem",
          transport: "http",
          config: { endpoint: "http://127.0.0.1:9100/mcp" },
          secret_ref: "env://MCP_TOKEN"
        });
        return Response.json(resourceDto("20000000-0000-4000-8000-000000000002", "filesystem"));
      }
      if (url.pathname === "/api/v1/mcp-servers/20000000-0000-4000-8000-000000000002") {
        expect(init?.method).toBe("PATCH");
        expect(jsonBody(init)).toMatchObject({
          tenant_id: "10000000-0000-4000-8000-000000000001",
          name: "filesystem-local",
          transport: "streamable-http",
          config: { endpoint: "http://127.0.0.1:9101/mcp" }
        });
        return Response.json(
          resourceDto("20000000-0000-4000-8000-000000000002", "filesystem-local")
        );
      }
      if (
        url.pathname ===
        "/api/v1/mcp-servers/20000000-0000-4000-8000-000000000002/tools:discover"
      ) {
        expect(init?.method).toBe("POST");
        expect(jsonBody(init)).toEqual({
          tenant_id: "10000000-0000-4000-8000-000000000001"
        });
        return Response.json([
          resourceDto("40000000-0000-4000-8000-000000000001", "read_file")
        ]);
      }
      if (url.pathname === "/api/v1/mcp-tools/40000000-0000-4000-8000-000000000001") {
        expect(init?.method).toBe("PATCH");
        expect(jsonBody(init)).toMatchObject({
          tenant_id: "10000000-0000-4000-8000-000000000001",
          name: "read_file_v2",
          schema_hash: "sha256:next"
        });
        return Response.json(resourceDto("40000000-0000-4000-8000-000000000001", "read_file_v2"));
      }
      if (
        url.pathname === "/api/v1/mcp-tools/40000000-0000-4000-8000-000000000001/disable"
      ) {
        expect(init?.method).toBe("POST");
        expect(jsonBody(init)).toEqual({
          tenant_id: "10000000-0000-4000-8000-000000000001"
        });
        return Response.json(
          resourceDto("40000000-0000-4000-8000-000000000001", "read_file_v2", "disabled")
        );
      }
      throw new Error(`unexpected path ${url.pathname}`);
    });
    const catalogApi = createCatalogApi(
      createHttpClient({
        baseUrl: "http://localhost:8361/api/v1",
        tokenProvider,
        fetchImpl: fetchImpl as typeof fetch
      })
    );

    const created = await catalogApi.createMcpServer({
      tenantId: "10000000-0000-4000-8000-000000000001",
      name: "filesystem",
      transport: "http",
      config: { endpoint: "http://127.0.0.1:9100/mcp" },
      secretRef: "env://MCP_TOKEN"
    });
    const updated = await catalogApi.updateMcpServer({
      tenantId: "10000000-0000-4000-8000-000000000001",
      mcpServerId: created.id,
      name: "filesystem-local",
      transport: "streamable-http",
      config: { endpoint: "http://127.0.0.1:9101/mcp" }
    });
    const discovered = await catalogApi.discoverMcpTools({
      tenantId: "10000000-0000-4000-8000-000000000001",
      mcpServerId: updated.id
    });
    const updatedTool = await catalogApi.updateMcpTool({
      tenantId: "10000000-0000-4000-8000-000000000001",
      mcpToolId: discovered[0].id,
      name: "read_file_v2",
      schemaHash: "sha256:next"
    });
    const disabledTool = await catalogApi.disableMcpTool({
      tenantId: "10000000-0000-4000-8000-000000000001",
      mcpToolId: discovered[0].id
    });

    expect(updated.name).toBe("filesystem-local");
    expect(discovered[0].name).toBe("read_file");
    expect(updatedTool.name).toBe("read_file_v2");
    expect(disabledTool.status).toBe("disabled");
  });

  it("creates, disables, and publishes versioned catalog resources", async () => {
    const fetchImpl = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = new URL(String(input));
      if (url.pathname === "/api/v1/agents") {
        expect(init?.method).toBe("POST");
        expect(jsonBody(init)).toMatchObject({
          tenant_id: "10000000-0000-4000-8000-000000000001",
          name: "researcher",
          draft_config: { model_profile_id: "local-profile" },
          metadata: { team: "platform" }
        });
        return Response.json(resourceDto("20000000-0000-4000-8000-000000000003", "researcher"));
      }
      if (url.pathname === "/api/v1/tools") {
        expect(jsonBody(init)).toMatchObject({
          tool_type: "http",
          schema: { input: "json" }
        });
        return Response.json(resourceDto("20000000-0000-4000-8000-000000000004", "webhook"));
      }
      if (url.pathname === "/api/v1/agents/20000000-0000-4000-8000-000000000003/versions") {
        expect(init?.method).toBe("POST");
        expect(jsonBody(init)).toMatchObject({
          version_label: "v1",
          snapshot: { model_profile_id: "local-profile" },
          policy_version: "local-v1"
        });
        return Response.json(versionDto("30000000-0000-4000-8000-000000000002"));
      }
      if (url.pathname === "/api/v1/agent-versions/30000000-0000-4000-8000-000000000002/disable") {
        expect(init?.method).toBe("POST");
        expect(jsonBody(init)).toEqual({
          tenant_id: "10000000-0000-4000-8000-000000000001"
        });
        return Response.json(versionDto("30000000-0000-4000-8000-000000000002", "disabled"));
      }
      if (url.pathname === "/api/v1/agents/20000000-0000-4000-8000-000000000003/disable") {
        expect(init?.method).toBe("POST");
        return Response.json(
          resourceDto("20000000-0000-4000-8000-000000000003", "researcher", "disabled")
        );
      }
      throw new Error(`unexpected path ${url.pathname}`);
    });
    const catalogApi = createCatalogApi(
      createHttpClient({
        baseUrl: "http://localhost:8361/api/v1",
        tokenProvider,
        fetchImpl: fetchImpl as typeof fetch
      })
    );

    const agent = await catalogApi.createResource({
      tenantId: "10000000-0000-4000-8000-000000000001",
      kind: "agents",
      name: "researcher",
      draftConfig: { model_profile_id: "local-profile" },
      metadata: { team: "platform" }
    });
    const tool = await catalogApi.createResource({
      tenantId: "10000000-0000-4000-8000-000000000001",
      kind: "tools",
      name: "webhook",
      toolType: "http",
      schema: { input: "json" }
    });
    const version = await catalogApi.publishVersion({
      tenantId: "10000000-0000-4000-8000-000000000001",
      kind: "agents",
      resourceId: agent.id,
      versionLabel: "v1",
      snapshot: { model_profile_id: "local-profile" },
      policyVersion: "local-v1"
    });
    const disabledVersion = await catalogApi.disableVersion({
      tenantId: "10000000-0000-4000-8000-000000000001",
      kind: "agents",
      versionId: version.id,
      resourceId: agent.id
    });
    const disabledAgent = await catalogApi.disableResource({
      tenantId: "10000000-0000-4000-8000-000000000001",
      kind: "agents",
      resourceId: agent.id
    });

    expect(tool.name).toBe("webhook");
    expect(version).toMatchObject({ versionLabel: "v1", status: "published" });
    expect(disabledVersion.status).toBe("disabled");
    expect(disabledAgent.status).toBe("disabled");
  });

  it("manages policy bindings and agent version checks through public API", async () => {
    const fetchImpl = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = new URL(String(input));
      if (url.pathname === "/api/v1/policy-bindings") {
        expect(init?.method).toBe("POST");
        expect(jsonBody(init)).toMatchObject({
          tenant_id: "10000000-0000-4000-8000-000000000001",
          resource_type: "agent",
          resource_id: "20000000-0000-4000-8000-000000000003",
          action: "execute",
          subject_type: "role",
          subject_id: "agent_runner",
          effect: "review",
          risk_level: "medium",
          obligations: { audit_level: "high" }
        });
        return Response.json(policyDto("60000000-0000-4000-8000-000000000001"));
      }
      if (url.pathname === "/api/v1/policy-bindings/60000000-0000-4000-8000-000000000001/disable") {
        expect(init?.method).toBe("POST");
        expect(jsonBody(init)).toEqual({
          tenant_id: "10000000-0000-4000-8000-000000000001"
        });
        return Response.json(policyDto("60000000-0000-4000-8000-000000000001", true));
      }
      if (
        url.pathname ===
        "/api/v1/agent-versions/30000000-0000-4000-8000-000000000002/effective-capabilities"
      ) {
        expect(url.searchParams.get("tenant_id")).toBe("10000000-0000-4000-8000-000000000001");
        return Response.json({
          agent_version_id: "30000000-0000-4000-8000-000000000002",
          tenant_id: "10000000-0000-4000-8000-000000000001",
          agent_id: "20000000-0000-4000-8000-000000000003",
          version_label: "v1",
          status: "published",
          policy_version: "local-v1",
          config_snapshot: { model_profile_id: "local-profile" },
          skills: [capabilityDto("skill", "70000000-0000-4000-8000-000000000001", "research")],
          tools: [],
          mcp_tools: []
        });
      }
      if (url.pathname === "/api/v1/agent-versions/30000000-0000-4000-8000-000000000002/validate") {
        expect(init?.method).toBe("POST");
        expect(jsonBody(init)).toEqual({
          tenant_id: "10000000-0000-4000-8000-000000000001"
        });
        return Response.json({
          valid: true,
          errors: [],
          warnings: ["agent version has no skill bindings"]
        });
      }
      throw new Error(`unexpected path ${url.pathname}`);
    });
    const catalogApi = createCatalogApi(
      createHttpClient({
        baseUrl: "http://localhost:8361/api/v1",
        tokenProvider,
        fetchImpl: fetchImpl as typeof fetch
      })
    );

    const binding = await catalogApi.createPolicyBinding({
      tenantId: "10000000-0000-4000-8000-000000000001",
      resourceType: "agent",
      resourceId: "20000000-0000-4000-8000-000000000003",
      action: "execute",
      subjectType: "role",
      subjectId: "agent_runner",
      effect: "review",
      riskLevel: "medium",
      obligations: { audit_level: "high" }
    });
    const disabled = await catalogApi.disablePolicyBinding({
      tenantId: "10000000-0000-4000-8000-000000000001",
      bindingId: binding.id
    });
    const capabilities = await catalogApi.getAgentVersionCapabilities({
      tenantId: "10000000-0000-4000-8000-000000000001",
      agentVersionId: "30000000-0000-4000-8000-000000000002"
    });
    const validation = await catalogApi.validateAgentVersion({
      tenantId: "10000000-0000-4000-8000-000000000001",
      agentVersionId: "30000000-0000-4000-8000-000000000002"
    });

    expect(binding.effect).toBe("review");
    expect(disabled.disabledAt).toBe("2026-06-22T00:00:00Z");
    expect(capabilities.skills[0]).toMatchObject({ name: "research", resourceType: "skill" });
    expect(validation).toMatchObject({
      valid: true,
      warnings: ["agent version has no skill bindings"]
    });
  });
});

function resourceDto(id: string, name: string, status = "active") {
  return {
    id,
    tenant_id: "10000000-0000-4000-8000-000000000001",
    name,
    description: null,
    status,
    metadata: { has_secret_ref: true },
    created_at: "2026-06-22T00:00:00Z",
    updated_at: "2026-06-22T00:00:00Z"
  };
}

function versionDto(id: string, status = "published") {
  return {
    id,
    tenant_id: "10000000-0000-4000-8000-000000000001",
    parent_id: "20000000-0000-4000-8000-000000000003",
    version_label: "v1",
    snapshot: { model_profile_id: "local-profile" },
    policy_version: "local-v1",
    status,
    created_at: "2026-06-22T00:00:00Z"
  };
}

function policyDto(id: string, disabled = false) {
  return {
    id,
    tenant_id: "10000000-0000-4000-8000-000000000001",
    resource_type: "agent",
    resource_id: "20000000-0000-4000-8000-000000000003",
    action: "execute",
    subject_type: "role",
    subject_id: "agent_runner",
    effect: "review",
    risk_level: "medium",
    obligations: { audit_level: "high" },
    policy_version: "local-v1",
    created_by_user_id: null,
    created_at: "2026-06-22T00:00:00Z",
    disabled_at: disabled ? "2026-06-22T00:00:00Z" : null
  };
}

function capabilityDto(resourceType: string, id: string, name: string) {
  return {
    resource_type: resourceType,
    resource_id: id,
    version_id: "71000000-0000-4000-8000-000000000001",
    parent_id: id,
    name,
    description: null,
    status: "published",
    snapshot: { mode: "default" },
    schema_hash: null,
    content_hash: null,
    source_uri: null
  };
}

function jsonBody(init?: RequestInit) {
  return JSON.parse(String(init?.body ?? "{}")) as unknown;
}
