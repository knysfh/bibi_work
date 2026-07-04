from __future__ import annotations

from uuid import uuid4

from bibi_work_agent.backends.platform_composite_backend import PlatformCompositeBackend
from bibi_work_agent.runtime.agent_factory import (
    build_platform_tools,
    build_runtime_chat_model,
    build_system_prompt,
    local_mount_id_for,
    resolve_runtime_model_credentials,
    tool_spec_ui_hints,
)


class FakeRust:
    def __init__(self) -> None:
        self.authorize_payloads: list = []
        self.local_exec_payloads: list[dict] = []
        self.mcp_payloads: list[dict] = []
        self.sql_payloads: list[dict] = []
        self.third_party_payloads: list[dict] = []

    def authorize_tool(self, payload):
        self.authorize_payloads.append(payload)
        return {"decision": {"decision": "allow"}}

    def local_exec_request(self, payload):
        self.local_exec_payloads.append(payload)
        return {"id": "local-exec-1", "status": "queued"}

    def mcp_tool_call(self, payload):
        self.mcp_payloads.append(payload)
        return {"status": "ok", "content": "mcp-result"}

    def sql_tool_execute(self, payload):
        self.sql_payloads.append(payload)
        return {"rows": [{"count": 1}], "row_count": 1}

    def third_party_tool_call(self, payload):
        self.third_party_payloads.append(payload)
        return {"status": "ok", "result": "third-party-result"}


class FakeBackend(PlatformCompositeBackend):
    def __init__(self) -> None:
        super().__init__(thread_id="thread")
        self.read_paths: list[str] = []
        self.list_prefixes: list[str | None] = []
        self.search_prefixes: list[str | None] = []

    def read_text(self, path: str) -> str:
        self.read_paths.append(path)
        return "content"

    def list_files(self, prefix: str | None = None) -> dict:
        self.list_prefixes.append(prefix)
        return {"files": [{"path": "/workspace/a.txt"}]}

    def search_files(
        self,
        query: str,
        *,
        prefix: str | None = None,
        limit: int = 50,
    ) -> dict:
        self.search_prefixes.append(prefix)
        return {"query": query, "limit": limit, "files": []}


def test_tool_spec_ui_hints_reads_output_schema_contract() -> None:
    assert tool_spec_ui_hints(
        {
            "name": "query_sales",
            "output_schema": {
                "type": "array",
                "items": {"type": "object"},
                "x-ui-hints": {"renderer": "data-grid"},
            },
        }
    ) == {"view": "table"}


def test_build_platform_tools_wraps_declared_tools_with_authorization() -> None:
    tenant_id = str(uuid4())
    user_id = str(uuid4())
    run_id = str(uuid4())
    rust = FakeRust()
    backend = FakeBackend()

    tools = build_platform_tools(
        {
            "tenant_id": tenant_id,
            "run_id": run_id,
            "actor": {"user_id": user_id},
            "tools": [{"name": "read_file"}],
        },
        backend=backend,
        rust=rust,
    )

    assert len(tools) == 1
    assert tools[0].__name__ == "read_file"
    assert tools[0](path="/workspace/a.txt") == "content"
    assert backend.read_paths == ["/workspace/a.txt"]
    assert rust.authorize_payloads[0].tool_name == "read_file"
    assert str(rust.authorize_payloads[0].tenant_id) == tenant_id
    assert str(rust.authorize_payloads[0].actor.user_id) == user_id
    assert str(rust.authorize_payloads[0].run_id) == run_id


def test_build_platform_tools_exposes_list_files() -> None:
    tenant_id = str(uuid4())
    user_id = str(uuid4())
    rust = FakeRust()
    backend = FakeBackend()

    tools = build_platform_tools(
        {
            "tenant_id": tenant_id,
            "actor": {"user_id": user_id},
            "tools": [{"name": "list_files"}],
        },
        backend=backend,
        rust=rust,
    )

    assert tools[0](prefix="/workspace/docs/") == {
        "files": [{"path": "/workspace/a.txt"}]
    }
    assert backend.list_prefixes == ["/workspace/docs/"]
    assert rust.authorize_payloads[0].tool_name == "list_files"


def test_build_platform_tools_lets_backend_choose_default_file_prefix() -> None:
    tenant_id = str(uuid4())
    user_id = str(uuid4())
    rust = FakeRust()
    backend = FakeBackend()

    list_tool, search_tool = build_platform_tools(
        {
            "tenant_id": tenant_id,
            "actor": {"user_id": user_id},
            "tools": [{"name": "list_files"}, {"name": "search_files"}],
        },
        backend=backend,
        rust=rust,
    )

    assert list_tool() == {"files": [{"path": "/workspace/a.txt"}]}
    assert search_tool(query="needle") == {"query": "needle", "limit": 50, "files": []}
    assert backend.list_prefixes == [None]
    assert backend.search_prefixes == [None]


def test_build_platform_tools_routes_local_exec_through_rust() -> None:
    tenant_id = str(uuid4())
    user_id = str(uuid4())
    project_id = str(uuid4())
    rust = FakeRust()
    backend = FakeBackend()

    tools = build_platform_tools(
        {
            "tenant_id": tenant_id,
            "project_id": project_id,
            "actor": {"user_id": user_id},
            "tools": [{"name": "local_exec"}],
        },
        backend=backend,
        rust=rust,
    )

    assert tools[0](command=["git", "status"], timeout_ms=1000) == {
        "id": "local-exec-1",
        "status": "queued",
    }
    assert rust.authorize_payloads[0].tool_name == "local_exec"
    payload = rust.local_exec_payloads[0]
    assert payload["tenant_id"] == tenant_id
    assert payload["actor_user_id"] == user_id
    assert payload["project_id"] == project_id
    assert payload["command"] == {"argv": ["git", "status"]}
    assert payload["timeout_ms"] == 1000


def test_build_platform_tools_routes_mcp_and_sql_through_rust() -> None:
    tenant_id = str(uuid4())
    user_id = str(uuid4())
    rust = FakeRust()
    backend = FakeBackend()

    mcp_tool, sql_tool = build_platform_tools(
        {
            "tenant_id": tenant_id,
            "actor": {"user_id": user_id},
            "tools": [{"name": "mcp_call"}, {"name": "sql_execute"}],
        },
        backend=backend,
        rust=rust,
    )

    assert mcp_tool(
        server_id="server-1", tool_name="lookup", arguments={"q": "sales"}
    ) == {
        "status": "ok",
        "content": "mcp-result",
    }
    assert sql_tool(sql_tool_id="sql-tool-1", query_hash="sha256:query") == {
        "rows": [{"count": 1}],
        "row_count": 1,
    }
    assert [payload.tool_name for payload in rust.authorize_payloads] == [
        "mcp_call",
        "sql_execute",
    ]
    assert rust.mcp_payloads[0]["tool_name"] == "lookup"
    assert rust.sql_payloads[0]["query_hash"] == "sha256:query"


def test_build_platform_tools_routes_third_party_tools_through_rust() -> None:
    tenant_id = str(uuid4())
    user_id = str(uuid4())
    rust = FakeRust()
    backend = FakeBackend()

    tools = build_platform_tools(
        {
            "tenant_id": tenant_id,
            "actor": {"user_id": user_id},
            "tools": [{"name": "third_party_call"}],
        },
        backend=backend,
        rust=rust,
    )

    assert tools[0](tool_id="tool-1", arguments={"q": "sales"}) == {
        "status": "ok",
        "result": "third-party-result",
    }
    assert rust.authorize_payloads[0].tool_name == "third_party_call"
    assert rust.third_party_payloads[0] == {
        "tenant_id": tenant_id,
        "actor": {
            "user_id": user_id,
            "device_id": None,
            "session_id": None,
            "roles": [],
        },
        "conversation_id": None,
        "run_id": None,
        "tool_id": "tool-1",
        "tool_version_id": None,
        "tool_name": None,
        "arguments": {"q": "sales"},
    }


def test_build_system_prompt_appends_memory_context_as_untrusted() -> None:
    prompt = build_system_prompt(
        {
            "system_prompt": "Follow platform policy.",
            "memory_context": [
                {
                    "memory_id": "memory-1",
                    "layer": "semantic",
                    "content": "Monthly sales use net revenue.",
                    "score": 0.9,
                    "sensitivity": "normal",
                }
            ],
        }
    )

    assert prompt.startswith("Follow platform policy.")
    assert "<untrusted_memory_context>" in prompt
    assert "Monthly sales use net revenue." in prompt


def test_build_system_prompt_describes_local_mount_current_directory() -> None:
    prompt = build_system_prompt(
        {
            "system_prompt": "Follow platform policy.",
            "workspace": {
                "remote_project_id": None,
                "local_mounts": [{"virtual_path": "/local/main/"}],
            },
        }
    )

    assert prompt.startswith("Follow platform policy.")
    assert "Mounted local folder root: /local/main/." in prompt
    assert "current workspace directory" in prompt
    assert "no project_id" in prompt
    assert "Do not use real OS paths such as /home or /Users." in prompt


def test_legacy_local_mount_root_maps_to_local_main() -> None:
    mount_id = str(uuid4())
    snapshot = {
        "system_prompt": "Follow platform policy.",
        "workspace": {
            "remote_project_id": None,
            "local_mounts": [{"local_mount_id": mount_id, "virtual_path": "/local/"}],
        },
    }

    prompt = build_system_prompt(snapshot)

    assert local_mount_id_for(snapshot, "/local/main/") == mount_id
    assert "Mounted local folder root: /local/main/." in prompt
    assert "no project_id" in prompt


def test_resolve_runtime_model_credentials_fetches_short_lived_secret() -> None:
    class FakeRuntimeCredentialRust:
        def __init__(self) -> None:
            self.calls: list[dict[str, str]] = []

        def runtime_credential(self, **kwargs):
            self.calls.append(kwargs)
            return {
                "auth_scheme": "bearer",
                "secret": "sk-test",
            }

    tenant_id = str(uuid4())
    run_id = str(uuid4())
    rust = FakeRuntimeCredentialRust()

    model = resolve_runtime_model_credentials(
        {
            "provider": "openai_compatible",
            "model_name": "gpt-test",
            "credential": {
                "credential_id": str(uuid4()),
                "has_secret_ref": True,
                "runtime_credential_id": "runtime-1",
            },
        },
        {"tenant_id": tenant_id, "run_id": run_id},
        rust=rust,
    )

    assert model["api_key"] == "sk-test"
    assert model["model"] == "gpt-test"
    assert "secret" not in model["credential"]
    assert rust.calls == [
        {
            "tenant_id": tenant_id,
            "run_id": run_id,
            "runtime_credential_id": "runtime-1",
        }
    ]


def test_build_runtime_chat_model_supports_openai_compatible_config() -> None:
    model = build_runtime_chat_model(
        {
            "provider": "openai-compatible",
            "model_name": "minimax-m2.5",
            "base_url": "http://llm.example.test",
            "api_key": "sk-test",
            "parameters": {
                "temperature": 0.2,
                "top_p": 0.8,
                "max_output_tokens": 128,
            },
        }
    )

    assert model.__class__.__name__ == "ChatOpenAI"
    assert model.model_name == "minimax-m2.5"
    assert str(model.openai_api_base) == "http://llm.example.test"
    assert model.temperature == 0.2
    assert model.top_p == 0.8
    assert model.max_tokens == 128
