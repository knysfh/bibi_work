from __future__ import annotations

import inspect
from functools import wraps
from typing import Any

from bibi_work_agent.api.schemas import ActorRef
from bibi_work_agent.backends.platform_composite_backend import PlatformCompositeBackend
from bibi_work_agent.clients.rust_client import RustClient


class PlatformToolAdapters:
    """Small tool implementations that delegate side effects back to Rust."""

    def __init__(
        self,
        *,
        rust: RustClient,
        tenant_id: str,
        actor: ActorRef,
        conversation_id: str | None,
        run_id: str | None,
        project_id: str | None,
        backend: PlatformCompositeBackend,
    ) -> None:
        self.rust = rust
        self.tenant_id = tenant_id
        self.actor = actor
        self.conversation_id = conversation_id
        self.run_id = run_id
        self.project_id = project_id
        self.backend = backend

    def callable_for(self, tool_name: str) -> Any:
        if tool_name in {"read_file", "file_read"}:
            return named_tool(tool_name, self.read_file)
        if tool_name in {"write_file", "file_write"}:
            return named_tool(tool_name, self.write_file)
        if tool_name in {"list_files", "file_list"}:
            return named_tool(tool_name, self.list_files)
        if tool_name in {"search_files", "file_search"}:
            return named_tool(tool_name, self.search_files)
        if tool_name.startswith(("mcp.", "mcp_")) or tool_name in {
            "mcp_call",
            "call_mcp_tool",
        }:
            return named_tool(tool_name, self.mcp_call)
        if tool_name in {"local_exec", "local_command", "run_local_command"}:
            return named_tool(tool_name, self.local_exec)
        if tool_name in {"sql_query", "sql_execute", "execute_sql_tool"}:
            return named_tool(tool_name, self.sql_execute)
        if tool_name in {
            "third_party_call",
            "third_party_tool",
            "http_tool",
            "external_tool",
        }:
            return named_tool(tool_name, self.third_party_call)

        def unsupported_tool(**_: Any) -> None:
            raise RuntimeError(
                f"platform tool implementation is not configured: {tool_name}"
            )

        return named_tool(tool_name, unsupported_tool)

    def read_file(self, path: str) -> str:
        return self.backend.read_text(path)

    def write_file(
        self,
        path: str,
        content: str,
        expected_revision: int,
        reason: str = "agent write",
    ) -> dict[str, Any] | None:
        return self.backend.write_text(
            path,
            content,
            expected_revision=expected_revision,
            reason=reason,
        )

    def list_files(self, prefix: str | None = None) -> dict[str, Any]:
        return self.backend.list_files(prefix)

    def search_files(
        self,
        query: str,
        prefix: str | None = None,
        limit: int = 50,
    ) -> dict[str, Any]:
        return self.backend.search_files(query, prefix=prefix, limit=limit)

    def mcp_call(
        self,
        *,
        server_id: str | None = None,
        tool_name: str,
        arguments: dict[str, Any] | None = None,
    ) -> dict[str, Any]:
        return self.rust.mcp_tool_call(
            {
                "tenant_id": self.tenant_id,
                "actor": self.actor.model_dump(mode="json"),
                "conversation_id": self.conversation_id,
                "run_id": self.run_id,
                "mcp_server_id": server_id,
                "tool_name": tool_name,
                "arguments": arguments or {},
            }
        )

    def local_exec(
        self,
        command: list[str] | dict[str, Any],
        *,
        device_id: str | None = None,
        timeout_ms: int | None = None,
        max_output_bytes: int | None = None,
    ) -> dict[str, Any]:
        command_payload = command if isinstance(command, dict) else {"argv": command}
        return self.rust.local_exec_request(
            {
                "tenant_id": self.tenant_id,
                "actor_user_id": str(self.actor.user_id),
                "actor_device_id": str(self.actor.device_id)
                if self.actor.device_id
                else None,
                "actor_session_id": str(self.actor.session_id)
                if self.actor.session_id
                else None,
                "device_id": device_id,
                "project_id": self.project_id,
                "run_id": self.run_id,
                "command": command_payload,
                "timeout_ms": timeout_ms,
                "max_output_bytes": max_output_bytes,
            }
        )

    def sql_execute(
        self,
        *,
        sql_tool_id: str | None = None,
        query_hash: str | None = None,
        parameters: dict[str, Any] | None = None,
    ) -> dict[str, Any]:
        return self.rust.sql_tool_execute(
            {
                "tenant_id": self.tenant_id,
                "actor": self.actor.model_dump(mode="json"),
                "conversation_id": self.conversation_id,
                "run_id": self.run_id,
                "sql_tool_id": sql_tool_id,
                "query_hash": query_hash,
                "parameters": parameters or {},
            }
        )

    def third_party_call(
        self,
        *,
        tool_id: str | None = None,
        tool_version_id: str | None = None,
        tool_name: str | None = None,
        arguments: dict[str, Any] | None = None,
    ) -> dict[str, Any]:
        return self.rust.third_party_tool_call(
            {
                "tenant_id": self.tenant_id,
                "actor": self.actor.model_dump(mode="json"),
                "conversation_id": self.conversation_id,
                "run_id": self.run_id,
                "tool_id": tool_id,
                "tool_version_id": tool_version_id,
                "tool_name": tool_name,
                "arguments": arguments or {},
            }
        )


def named_tool(tool_name: str, func: Any) -> Any:
    @wraps(func)
    def tool(*args: Any, **kwargs: Any) -> Any:
        return func(*args, **kwargs)

    tool.__name__ = tool_name
    tool.__signature__ = inspect.signature(func)
    return tool
