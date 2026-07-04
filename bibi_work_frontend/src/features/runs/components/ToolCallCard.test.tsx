import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { PlatformProviders } from "../../../app/providers";
import { ToolCallCard } from "./ToolCallCard";

describe("ToolCallCard", () => {
  afterEach(() => {
    cleanup();
    window.sessionStorage.clear();
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it("renders structured table views after the tool card is expanded", async () => {
    const user = userEvent.setup();
    const { container } = render(
      <PlatformProviders>
        <ToolCallCard
          toolCall={{
            id: "tool-1",
            name: "query_sales",
            status: "completed",
            outputSummary: "ok",
            views: [
              {
                kind: "table",
                columns: [{ key: "region", label: "区域", type: "string" }],
                rowsPreview: [{ region: "华东" }]
              }
            ]
          }}
        />
      </PlatformProviders>
    );

    const details = container.querySelector("details");
    expect(details).not.toHaveAttribute("open");

    expect(screen.getByText("query_sales")).toBeInTheDocument();
    await user.click(screen.getByText("query_sales"));
    expect(screen.getByText("区域")).toBeInTheDocument();
    expect(screen.getByText("华东")).toBeInTheDocument();
  });

  it("allows users to collapse and expand tool details", async () => {
    const user = userEvent.setup();
    const { container } = render(
      <PlatformProviders>
        <ToolCallCard
          toolCall={{
            id: "tool-1",
            name: "ls",
            status: "completed",
            outputSummary: "ok",
            views: [
              {
                kind: "table",
                columns: [{ key: "path", label: "path", type: "string" }],
                rowsPreview: [{ path: "/local/main/readme.md" }]
              }
            ]
          }}
        />
      </PlatformProviders>
    );

    const details = container.querySelector("details");
    expect(details).not.toHaveAttribute("open");

    await user.click(screen.getByText("ls"));
    expect(details).toHaveAttribute("open");

    await user.click(screen.getByText("ls"));
    expect(details).not.toHaveAttribute("open");
  });

  it("loads artifact-backed table rows from a replayed tool result", async () => {
    window.sessionStorage.setItem(
      "bibi_work.token_set",
      JSON.stringify({ accessToken: "test-token" })
    );
    const fetchImpl = vi.fn(async (input: RequestInfo | URL) => {
      const url = new URL(String(input));
      if (url.pathname.endsWith("/me")) {
        return Response.json(meDto());
      }
      if (url.pathname.endsWith("/tool-result-artifacts/read")) {
        expect(url.searchParams.get("offset")).toBe("0");
        expect(url.searchParams.get("limit")).toBe("500");
        return Response.json(toolResultArtifactDto());
      }
      return Response.json({}, { status: 404 });
    });
    vi.stubGlobal("fetch", fetchImpl);
    const user = userEvent.setup();

    render(
      <PlatformProviders>
        <ToolCallCard
          toolCall={{
            id: "tool-1",
            name: "query_sales",
            status: "completed",
            outputSummary: "ok",
            views: [
              {
                kind: "table",
                columns: [{ key: "region", label: "区域", type: "string" }],
                rowsPreview: [{ region: "预览" }],
                dataRef: {
                  artifactId: "artifact-1",
                  objectReferenceId: "50000000-0000-4000-8000-000000000001",
                  contentType: "application/x-ndjson",
                  contentHash: "sha256:abc",
                  sizeBytes: 32
                }
              }
            ]
          }}
        />
      </PlatformProviders>
    );

    await user.click(screen.getByText("query_sales"));
    await user.click(screen.getByRole("button", { name: "Load artifact" }));

    await waitFor(() => expect(screen.getByText("已加载")).toBeInTheDocument());
    expect(fetchImpl).toHaveBeenCalledWith(
      expect.stringContaining("/tool-result-artifacts/read"),
      expect.any(Object)
    );
  });
});

function meDto() {
  return {
    tenant_id: "10000000-0000-4000-8000-000000000001",
    user: {
      id: "10000000-0000-4000-8000-000000000002",
      tenant_id: "10000000-0000-4000-8000-000000000001",
      ferriskey_subject: "user-1",
      username: "tester",
      email: null,
      display_name: null,
      status: "active",
      created_at: "2026-06-22T00:00:00Z",
      updated_at: "2026-06-22T00:00:00Z"
    },
    tenants: [
      {
        id: "10000000-0000-4000-8000-000000000001",
        name: "Test Tenant",
        slug: "test",
        membership_role: "tenant_member",
        metadata: {}
      }
    ],
    roles: ["tenant_member"],
    capabilities: [],
    device: {
      id: "10000000-0000-4000-8000-000000000003",
      tenant_id: "10000000-0000-4000-8000-000000000001",
      device_name: "browser",
      platform: "test",
      trust_level: "trusted",
      last_seen_at: null,
      revoked_at: null
    },
    session: {
      id: "10000000-0000-4000-8000-000000000004",
      tenant_id: "10000000-0000-4000-8000-000000000001",
      device_id: "10000000-0000-4000-8000-000000000003",
      token_exp: "2026-06-22T01:00:00Z",
      last_seen_at: null,
      source_ip: null,
      user_agent: null,
      revoked_at: null
    }
  };
}

function toolResultArtifactDto() {
  return {
    id: "60000000-0000-4000-8000-000000000001",
    tenant_id: "10000000-0000-4000-8000-000000000001",
    run_id: null,
    tool_call_id: null,
    view_kind: "table",
    ref_kind: "data_ref",
    project_id: "20000000-0000-4000-8000-000000000001",
    path: "/artifacts/tool-results/result-table.jsonl",
    revision: 1,
    file_revision_id: "40000000-0000-4000-8000-000000000001",
    object_reference_id: "50000000-0000-4000-8000-000000000001",
    content_hash: "abc",
    content_type: "application/x-ndjson",
    size_bytes: 32,
    content: {
      kind: "json_rows",
      offset: 0,
      limit: 500,
      total_rows: 1,
      rows: [{ region: "已加载" }]
    },
    created_at: "2026-06-22T00:00:00Z"
  };
}
