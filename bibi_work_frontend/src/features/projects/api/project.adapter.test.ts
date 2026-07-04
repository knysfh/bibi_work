import { describe, expect, it, vi } from "vitest";
import { createHttpClient } from "../../../shared/api/http-client";
import type { TokenProvider } from "../../../shared/api/token-provider";
import { createProjectApi } from "./project.adapter";

const tokenProvider: TokenProvider = {
  getAccessToken: vi.fn(async () => "access-token"),
  setTokenSet: vi.fn(),
  clearTokenSet: vi.fn()
};

describe("project adapter", () => {
  it("searches project files through the public files:search API", async () => {
    const fetchImpl = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      expect(new URL(String(input)).pathname).toBe(
        "/api/v1/projects/20000000-0000-4000-8000-000000000001/files:search"
      );
      expect(JSON.parse(String(init?.body))).toMatchObject({
        tenant_id: "10000000-0000-4000-8000-000000000001",
        query: "policy",
        prefix: "/workspace/",
        limit: 25
      });
      return Response.json(fileListDto());
    });
    const projectApi = createProjectApi(
      createHttpClient({
        baseUrl: "http://localhost:8361/api/v1",
        tokenProvider,
        fetchImpl: fetchImpl as typeof fetch
      })
    );

    const result = await projectApi.searchFiles({
      tenantId: "10000000-0000-4000-8000-000000000001",
      projectId: "20000000-0000-4000-8000-000000000001",
      query: "policy",
      prefix: "/workspace/",
      limit: 25
    });

    expect(result.files[0]).toMatchObject({ path: "/workspace/policy.md", revision: 3 });
  });

  it("reads history and artifacts with stable query parameters", async () => {
    const fetchImpl = vi.fn(async (input: RequestInfo | URL) => {
      const url = new URL(String(input));
      if (url.pathname.endsWith("/files/history")) {
        expect(url.searchParams.get("path")).toBe("/workspace/policy.md");
        expect(url.searchParams.get("limit")).toBe("50");
      }
      if (url.pathname.endsWith("/artifacts")) {
        expect(url.searchParams.get("run_id")).toBe("30000000-0000-4000-8000-000000000001");
      }
      return Response.json(fileListDto());
    });
    const projectApi = createProjectApi(
      createHttpClient({
        baseUrl: "http://localhost:8361/api/v1",
        tokenProvider,
        fetchImpl: fetchImpl as typeof fetch
      })
    );

    await projectApi.listFileHistory({
      tenantId: "10000000-0000-4000-8000-000000000001",
      projectId: "20000000-0000-4000-8000-000000000001",
      path: "/workspace/policy.md"
    });
    await projectApi.listArtifacts({
      tenantId: "10000000-0000-4000-8000-000000000001",
      projectId: "20000000-0000-4000-8000-000000000001",
      runId: "30000000-0000-4000-8000-000000000001"
    });

    expect(fetchImpl).toHaveBeenCalledTimes(2);
  });

  it("reads tool result artifact pages by object reference", async () => {
    const fetchImpl = vi.fn(async (input: RequestInfo | URL) => {
      const url = new URL(String(input));
      expect(url.pathname).toBe("/api/v1/tool-result-artifacts/read");
      expect(url.searchParams.get("tenant_id")).toBe(
        "10000000-0000-4000-8000-000000000001"
      );
      expect(url.searchParams.get("object_reference_id")).toBe(
        "50000000-0000-4000-8000-000000000001"
      );
      expect(url.searchParams.get("offset")).toBe("50");
      expect(url.searchParams.get("limit")).toBe("50");
      return Response.json({
        id: "60000000-0000-4000-8000-000000000001",
        tenant_id: "10000000-0000-4000-8000-000000000001",
        run_id: null,
        tool_call_id: null,
        view_kind: "table",
        ref_kind: "data_ref",
        project_id: "20000000-0000-4000-8000-000000000001",
        path: "/artifacts/tool-results/result-table.json",
        revision: 1,
        file_revision_id: "40000000-0000-4000-8000-000000000001",
        object_reference_id: "50000000-0000-4000-8000-000000000001",
        content_hash: "abc",
        content_type: "application/json",
        size_bytes: 10,
        content: { kind: "json_rows", offset: 50, limit: 50, total_rows: 60, rows: [] },
        created_at: "2026-06-22T00:00:00Z"
      });
    });
    const projectApi = createProjectApi(
      createHttpClient({
        baseUrl: "http://localhost:8361/api/v1",
        tokenProvider,
        fetchImpl: fetchImpl as typeof fetch
      })
    );

    const result = await projectApi.readToolResultArtifact({
      tenantId: "10000000-0000-4000-8000-000000000001",
      objectReferenceId: "50000000-0000-4000-8000-000000000001",
      offset: 50,
      limit: 50
    });

    expect(result).toMatchObject({
      objectReferenceId: "50000000-0000-4000-8000-000000000001",
      viewKind: "table"
    });
  });
});

function fileListDto() {
  return {
    files: [
      {
        id: "40000000-0000-4000-8000-000000000001",
        tenant_id: "10000000-0000-4000-8000-000000000001",
        project_id: "20000000-0000-4000-8000-000000000001",
        path: "/workspace/policy.md",
        revision: 3,
        etag: "etag",
        content_hash: "hash",
        object_key: "objects/policy.md",
        object_reference_id: null,
        bucket: null,
        version_id: null,
        inline_content: "content",
        content_base64: null,
        size_bytes: 7,
        content_type: "text/markdown",
        is_binary: false,
        is_large: false,
        reason: "test",
        run_id: null,
        metadata: {},
        created_at: "2026-06-22T00:00:00Z"
      }
    ],
    entries: []
  };
}
