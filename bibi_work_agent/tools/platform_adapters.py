from __future__ import annotations

import hashlib
import inspect
import json
import keyword
from collections.abc import Callable
from functools import wraps
from typing import Any
from urllib.parse import urlparse
from uuid import uuid4

from bibi_work_agent.api.schemas import ActorRef
from bibi_work_agent.backends.platform_composite_backend import PlatformCompositeBackend
from bibi_work_agent.clients.rust_client import RustClient
from bibi_work_agent.runtime.cancellation import RunCancelled, is_run_cancelled


BROWSER_RECOVERY_ATTEMPTS_PER_FINGERPRINT = 2
MAX_TRACKED_BROWSER_FAILURES = 32


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
        self._browser_failure_attempts: dict[str, int] = {}

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
        browser_callable = {
            "browser_open": self.browser_open,
            "browser_goto": self.browser_goto,
            "browser_snapshot": self.browser_snapshot,
            "browser_tab_list": self.browser_tab_list,
            "browser_tab_open": self.browser_tab_open,
            "browser_tab_select": self.browser_tab_select,
            "browser_tab_close": self.browser_tab_close,
            "browser_click": self.browser_click,
            "browser_fill": self.browser_fill,
            "browser_press": self.browser_press,
            "browser_scroll": self.browser_scroll,
            "browser_wait_for_change": self.browser_wait_for_change,
            "browser_extract_text": self.browser_extract_text,
            "browser_wait_for_user": self.browser_wait_for_user,
            "browser_close": self.browser_close,
        }.get(tool_name)
        if browser_callable is not None:
            return named_tool(tool_name, browser_callable)
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
        self._raise_if_cancelled()
        return self.backend.read_text(path)

    def write_file(
        self,
        path: str,
        content: str,
        expected_revision: int,
        reason: str = "agent write",
    ) -> dict[str, Any] | None:
        self._raise_if_cancelled()
        return self.backend.write_text(
            path,
            content,
            expected_revision=expected_revision,
            reason=reason,
        )

    def list_files(self, prefix: str | None = None) -> dict[str, Any]:
        self._raise_if_cancelled()
        return self.backend.list_files(prefix)

    def search_files(
        self,
        query: str,
        prefix: str | None = None,
        limit: int = 50,
    ) -> dict[str, Any]:
        self._raise_if_cancelled()
        return self.backend.search_files(query, prefix=prefix, limit=limit)

    def mcp_call(
        self,
        *,
        server_id: str | None = None,
        mcp_tool_id: str | None = None,
        tool_name: str,
        arguments: dict[str, Any] | None = None,
    ) -> dict[str, Any]:
        return self._call_rust(
            self.rust.mcp_tool_call,
            {
                "tenant_id": self.tenant_id,
                "actor": self.actor.model_dump(mode="json"),
                "conversation_id": self.conversation_id,
                "run_id": self.run_id,
                "mcp_server_id": server_id,
                "mcp_tool_id": mcp_tool_id,
                "tool_name": tool_name,
                "arguments": arguments or {},
            },
        )

    def bound_mcp_tool(
        self,
        *,
        runtime_name: str,
        mcp_tool_id: str,
        tool_name: str,
        server_id: str | None = None,
        input_schema: dict[str, Any] | None = None,
    ) -> Any:
        def call_bound_mcp_tool(**arguments: Any) -> dict[str, Any]:
            return self.mcp_call(
                server_id=server_id,
                mcp_tool_id=mcp_tool_id,
                tool_name=tool_name,
                arguments=unwrap_inferred_argument_envelope(
                    arguments,
                    input_schema=input_schema,
                ),
            )

        return named_tool(runtime_name, call_bound_mcp_tool, input_schema=input_schema)

    def local_exec(
        self,
        command: list[str] | dict[str, Any],
        *,
        device_id: str | None = None,
        timeout_ms: int | None = None,
        max_output_bytes: int | None = None,
    ) -> dict[str, Any]:
        command_payload = command if isinstance(command, dict) else {"argv": command}
        return self._call_rust(
            self.rust.local_exec_request,
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
            },
        )

    def browser_open(
        self,
        url: str,
        session_id: str | None = None,
        profile: str = "default",
    ) -> dict[str, Any]:
        """Open a visible local browser and return its session id and page snapshot."""
        return self._browser_action(
            session_id=session_id or str(uuid4()),
            profile=profile,
            action={"name": "open", "url": normalize_browser_url(url)},
        )

    def browser_goto(self, session_id: str, url: str) -> dict[str, Any]:
        """Navigate an existing browser session to an HTTP(S) URL."""
        return self._browser_action(
            session_id=session_id,
            action={"name": "goto", "url": normalize_browser_url(url)},
        )

    def browser_snapshot(self, session_id: str) -> dict[str, Any]:
        """Read the current page and refresh stable interactive element references."""
        return self._browser_action(
            session_id=session_id,
            action={"name": "snapshot"},
        )

    def browser_tab_list(self, session_id: str) -> dict[str, Any]:
        """List open tabs with stable tab ids for the current browser session."""
        return self._browser_action(
            session_id=session_id,
            action={"name": "tab_list"},
        )

    def browser_tab_open(self, session_id: str, url: str) -> dict[str, Any]:
        """Open an HTTP(S) URL in a new tab and make it active."""
        return self._browser_action(
            session_id=session_id,
            action={"name": "tab_open", "url": normalize_browser_url(url)},
        )

    def browser_tab_select(self, session_id: str, tab_id: str) -> dict[str, Any]:
        """Select a tab id returned by browser_tab_list or a page snapshot."""
        return self._browser_action(
            session_id=session_id,
            action={"name": "tab_select", "tab_id": tab_id},
        )

    def browser_tab_close(self, session_id: str, tab_id: str) -> dict[str, Any]:
        """Close one tab without closing the whole browser session."""
        return self._browser_action(
            session_id=session_id,
            action={"name": "tab_close", "tab_id": tab_id},
        )

    def browser_click(self, session_id: str, ref: str) -> dict[str, Any]:
        """Click an element reference returned by the latest browser snapshot."""
        return self._browser_action(
            session_id=session_id,
            action={"name": "click", "ref": ref},
        )

    def browser_fill(self, session_id: str, ref: str, text: str) -> dict[str, Any]:
        """Fill a non-password field referenced by the latest browser snapshot."""
        return self._browser_action(
            session_id=session_id,
            action={"name": "fill", "ref": ref, "text": text},
        )

    def browser_press(self, session_id: str, key: str) -> dict[str, Any]:
        """Press a keyboard key in the active page."""
        return self._browser_action(
            session_id=session_id,
            action={"name": "press", "key": key},
        )

    def browser_scroll(
        self,
        session_id: str,
        delta_y: int = 700,
        ref: str | None = None,
        delta_x: int = 0,
    ) -> dict[str, Any]:
        """Scroll the page or a scrollable snapshot ref and return a fresh snapshot."""
        return self._browser_action(
            session_id=session_id,
            action={
                "name": "scroll",
                "ref": ref,
                "delta_x": delta_x,
                "delta_y": delta_y,
            },
        )

    def browser_wait_for_change(
        self,
        session_id: str,
        timeout_ms: int = 10_000,
    ) -> dict[str, Any]:
        """Wait for SPA content to change, settle, and return a fresh snapshot."""
        return self._browser_action(
            session_id=session_id,
            action={"name": "wait_for_change", "timeout_ms": timeout_ms},
        )

    def browser_extract_text(
        self,
        session_id: str,
        ref: str | None = None,
    ) -> dict[str, Any]:
        """Extract page text or text from a ref returned by the latest snapshot."""
        return self._browser_action(
            session_id=session_id,
            action={"name": "extract_text", "ref": ref},
        )

    def browser_wait_for_user(
        self,
        session_id: str,
        reason: str,
        expected_url: str | None = None,
    ) -> dict[str, Any]:
        """Pause for login, MFA, CAPTCHA, or another manual browser action."""
        return self._browser_action(
            session_id=session_id,
            action={
                "name": "wait_for_user",
                "reason": reason,
                "expected_url": expected_url,
            },
        )

    def browser_close(self, session_id: str) -> dict[str, Any]:
        """Close a local browser session while retaining its persistent profile."""
        return self._browser_action(
            session_id=session_id,
            action={"name": "close"},
        )

    def _browser_action(
        self,
        *,
        session_id: str,
        action: dict[str, Any],
        profile: str = "default",
        timeout_ms: int = 120_000,
    ) -> dict[str, Any]:
        if not self.actor.device_id:
            raise RuntimeError("local browser requires an authenticated desktop device")
        queued = self._call_rust(
            self.rust.local_exec_request,
            {
                "tenant_id": self.tenant_id,
                "actor_user_id": str(self.actor.user_id),
                "actor_device_id": str(self.actor.device_id),
                "actor_session_id": str(self.actor.session_id)
                if self.actor.session_id
                else None,
                "device_id": str(self.actor.device_id),
                "project_id": self.project_id,
                "run_id": self.run_id,
                "command": {
                    "protocol": "biwork_browser.v1",
                    "kind": "browser",
                    "session_id": session_id,
                    "profile": profile,
                    "action": action,
                },
                "timeout_ms": timeout_ms,
                "max_output_bytes": 1_048_576,
            },
        )
        request_id = queued.get("id")
        if not request_id:
            raise RuntimeError("browser request did not return an id")
        completed = self.rust.local_exec_wait(
            request_id=request_id,
            tenant_id=self.tenant_id,
            timeout_ms=timeout_ms,
        )
        self._raise_if_cancelled()
        if completed.get("status") != "completed":
            result = completed.get("result")
            if isinstance(result, dict) and result.get("retryable") is True:
                return self._register_browser_recovery(action, result)
            raise RuntimeError(completed.get("error") or "browser action failed")
        result = completed.get("result")
        if not isinstance(result, dict):
            raise RuntimeError("browser action returned an invalid result")
        if action.get("name") not in {
            "snapshot",
            "tab_list",
            "extract_text",
            "wait_for_user",
        }:
            self._browser_failure_attempts.clear()
        return result

    def _register_browser_recovery(
        self,
        action: dict[str, Any],
        result: dict[str, Any],
    ) -> dict[str, Any]:
        fingerprint = browser_failure_fingerprint(action, result)
        attempt = self._browser_failure_attempts.get(fingerprint, 0) + 1
        self._browser_failure_attempts[fingerprint] = attempt
        while len(self._browser_failure_attempts) > MAX_TRACKED_BROWSER_FAILURES:
            oldest = next(iter(self._browser_failure_attempts))
            self._browser_failure_attempts.pop(oldest, None)

        error = result.get("error")
        code = error.get("code") if isinstance(error, dict) else None
        action_name = str(action.get("name") or "unknown")
        if attempt > BROWSER_RECOVERY_ATTEMPTS_PER_FINGERPRINT:
            raise RuntimeError(
                "BROWSER_RECOVERY_EXHAUSTED: the same browser failure remained "
                "unresolved after two model recovery attempts "
                f"(code={code or 'unknown'}, action={action_name}, "
                f"fingerprint={fingerprint[:16]})."
            )

        recovered = dict(result)
        recovered["recovery"] = {
            "failure_fingerprint": fingerprint[:16],
            "attempt": attempt,
            "max_attempts_for_same_failure": (
                BROWSER_RECOVERY_ATTEMPTS_PER_FINGERPRINT
            ),
            "scope": (
                "same error code, action target, and observed browser state; "
                "a changed error, target, or page state starts a new recovery episode"
            ),
        }
        return recovered

    def sql_execute(
        self,
        *,
        sql_tool_id: str | None = None,
        query_hash: str | None = None,
        parameters: dict[str, Any] | None = None,
    ) -> dict[str, Any]:
        return self._call_rust(
            self.rust.sql_tool_execute,
            {
                "tenant_id": self.tenant_id,
                "actor": self.actor.model_dump(mode="json"),
                "conversation_id": self.conversation_id,
                "run_id": self.run_id,
                "sql_tool_id": sql_tool_id,
                "query_hash": query_hash,
                "parameters": parameters or {},
            },
        )

    def bound_sql_tool(
        self,
        *,
        runtime_name: str,
        sql_tool_id: str | None,
        query_hash: str | None = None,
    ) -> Any:
        def call_bound_sql_tool(**parameters: Any) -> dict[str, Any]:
            return self.sql_execute(
                sql_tool_id=sql_tool_id,
                query_hash=query_hash,
                parameters=parameters,
            )

        return named_tool(runtime_name, call_bound_sql_tool)

    def third_party_call(
        self,
        *,
        tool_id: str | None = None,
        tool_version_id: str | None = None,
        tool_name: str | None = None,
        arguments: dict[str, Any] | None = None,
    ) -> dict[str, Any]:
        return self._call_rust(
            self.rust.third_party_tool_call,
            {
                "tenant_id": self.tenant_id,
                "actor": self.actor.model_dump(mode="json"),
                "conversation_id": self.conversation_id,
                "run_id": self.run_id,
                "tool_id": tool_id,
                "tool_version_id": tool_version_id,
                "tool_name": tool_name,
                "arguments": arguments or {},
            },
        )

    def bound_third_party_tool(
        self,
        *,
        runtime_name: str,
        tool_id: str | None,
        tool_version_id: str | None,
        tool_name: str | None,
        input_schema: dict[str, Any] | None = None,
    ) -> Any:
        def call_bound_third_party_tool(**arguments: Any) -> dict[str, Any]:
            return self.third_party_call(
                tool_id=tool_id,
                tool_version_id=tool_version_id,
                tool_name=tool_name,
                arguments=unwrap_inferred_argument_envelope(
                    arguments,
                    input_schema=input_schema,
                ),
            )

        return named_tool(runtime_name, call_bound_third_party_tool)

    def _raise_if_cancelled(self) -> None:
        if self.run_id and is_run_cancelled(str(self.run_id)):
            raise RunCancelled(str(self.run_id))

    def _call_rust(
        self,
        call: Callable[[dict[str, Any]], dict[str, Any]],
        payload: dict[str, Any],
    ) -> dict[str, Any]:
        self._raise_if_cancelled()
        result = call(payload)
        self._raise_if_cancelled()
        return result


def browser_failure_fingerprint(action: dict[str, Any], result: dict[str, Any]) -> str:
    error = result.get("error")
    error_code = error.get("code") if isinstance(error, dict) else None
    snapshot = result.get("recovery_snapshot")
    state: dict[str, Any] | None = None
    if isinstance(snapshot, dict):
        elements = snapshot.get("elements")
        element_projection = []
        if isinstance(elements, list):
            for element in elements[:120]:
                if not isinstance(element, dict):
                    continue
                element_projection.append(
                    {
                        "ref": element.get("ref"),
                        "label": element.get("label"),
                        "tag": element.get("tag"),
                        "type": element.get("type"),
                        "frame_id": element.get("frame_id"),
                    }
                )
        state = {
            "url": normalized_browser_state_url(snapshot.get("url")),
            "title": snapshot.get("title"),
            "element_count": snapshot.get("element_count"),
            "elements": element_projection,
        }
    payload = {
        "error_code": error_code,
        "target": {
            "name": action.get("name"),
            "ref": action.get("ref"),
            "key": action.get("key"),
            "url": action.get("url"),
        },
        "state": state,
    }
    encoded = json.dumps(
        payload,
        ensure_ascii=True,
        sort_keys=True,
        separators=(",", ":"),
        default=str,
    )
    return hashlib.sha256(encoded.encode("utf-8")).hexdigest()


def normalized_browser_state_url(value: Any) -> str | None:
    if not isinstance(value, str) or not value:
        return None
    parsed = urlparse(value)
    if not parsed.scheme or not parsed.netloc:
        return value
    return f"{parsed.scheme}://{parsed.netloc}{parsed.path}"


def normalize_browser_url(value: str) -> str:
    value = value.strip()
    if not value:
        raise ValueError("browser URL is required")
    if "://" not in value:
        value = f"https://{value}"
    parsed = urlparse(value)
    if parsed.scheme not in {"http", "https"}:
        raise ValueError(
            "browser URL must use http:// or https://; view-source and local-file "
            "URLs are not supported, so use browser_snapshot to inspect the page"
        )
    if parsed.username or parsed.password:
        raise ValueError("browser URL must not contain embedded credentials")
    return value


def named_tool(
    tool_name: str,
    func: Any,
    *,
    input_schema: dict[str, Any] | None = None,
) -> Any:
    @wraps(func)
    def tool(*args: Any, **kwargs: Any) -> Any:
        return func(*args, **kwargs)

    tool.__name__ = tool_name
    tool.__doc__ = func.__doc__ or f"Execute the governed platform tool {tool_name}."
    tool.__signature__ = signature_from_json_schema(input_schema) or inspect.signature(
        func
    )
    return tool


def signature_from_json_schema(
    input_schema: dict[str, Any] | None,
) -> inspect.Signature | None:
    if not isinstance(input_schema, dict):
        return None
    properties = input_schema.get("properties")
    if not isinstance(properties, dict) or not properties:
        return None
    required = {
        value for value in input_schema.get("required", []) if isinstance(value, str)
    }
    parameters: list[inspect.Parameter] = []
    for name, schema in properties.items():
        if (
            not isinstance(name, str)
            or not name.isidentifier()
            or keyword.iskeyword(name)
        ):
            continue
        annotation = json_schema_annotation(schema)
        parameters.append(
            inspect.Parameter(
                name,
                inspect.Parameter.KEYWORD_ONLY,
                default=(inspect.Parameter.empty if name in required else None),
                annotation=annotation,
            )
        )
    if not parameters:
        return None
    return inspect.Signature(parameters)


def json_schema_annotation(schema: Any) -> Any:
    if not isinstance(schema, dict):
        return Any
    schema_type = schema.get("type")
    if schema_type == "string":
        return str
    if schema_type == "integer":
        return int
    if schema_type == "number":
        return float
    if schema_type == "boolean":
        return bool
    if schema_type == "array":
        return list[Any]
    if schema_type == "object":
        return dict[str, Any]
    return Any


def unwrap_inferred_argument_envelope(
    arguments: dict[str, Any],
    *,
    input_schema: dict[str, Any] | None,
) -> dict[str, Any]:
    """Remove the kwargs schema envelope unless it is a real tool input field."""
    properties = input_schema.get("properties") if input_schema else None
    if isinstance(properties, dict) and "arguments" in properties:
        return arguments
    nested = arguments.get("arguments")
    if len(arguments) == 1 and isinstance(nested, dict):
        return nested
    return arguments
