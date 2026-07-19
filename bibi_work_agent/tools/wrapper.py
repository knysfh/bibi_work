from __future__ import annotations

import hashlib
import json
from uuid import uuid4
from collections.abc import Callable, Mapping
from functools import wraps
from typing import Any

from langgraph.types import interrupt

from bibi_work_agent.api.schemas import ActorRef, ToolAuthorizeRequest
from bibi_work_agent.clients.rust_client import RustClient
from bibi_work_agent.runtime.cancellation import RunCancelled, is_run_cancelled
from bibi_work_agent.tools.io_policy import (
    apply_output_policy,
    summarize_input,
    summarize_output,
)
from bibi_work_agent.tools.result_presenter import build_tool_result_views
from bibi_work_agent.tools.risk import classify_tool_risk
from bibi_work_agent.runtime.event_normalizer import extract_path
from bibi_work_agent.runtime.tool_context import (
    current_file_tool_call,
    file_tool_call_context,
)


class ToolDenied(RuntimeError):
    pass


class ToolRequiresApproval(RuntimeError):
    def __init__(self, approval_id: str | None) -> None:
        super().__init__("tool execution requires approval")
        self.approval_id = approval_id


class PlatformToolWrapper:
    def __init__(
        self,
        *,
        rust: RustClient,
        tenant_id: str,
        actor: ActorRef,
        conversation_id: str | None,
        run_id: str | None,
        project_id: str | None = None,
        trace_id: str | None = None,
    ) -> None:
        self.rust = rust
        self.tenant_id = tenant_id
        self.actor = actor
        self.conversation_id = conversation_id
        self.run_id = run_id
        self.project_id = project_id
        self.trace_id = trace_id

    def wrap(
        self,
        tool_name: str,
        func: Callable[..., Any],
        *,
        ui_hints: dict[str, Any] | None = None,
        resource: dict[str, Any] | None = None,
        risk_level: str | None = None,
    ) -> Callable[..., Any]:
        @wraps(func)
        def wrapped(*args: Any, **kwargs: Any) -> Any:
            self._raise_if_cancelled()
            call_args = {"args": args, "kwargs": kwargs}
            args_hash = stable_hash(call_args)
            decision_payload = self.rust.authorize_tool(
                ToolAuthorizeRequest(
                    tenant_id=self.tenant_id,
                    actor=self.actor,
                    conversation_id=self.conversation_id,
                    run_id=self.run_id,
                    trace_id=self.trace_id,
                    tool_name=tool_name,
                    resource=tool_authz_resource(
                        tool_name,
                        kwargs,
                        static_resource=resource,
                        actor=self.actor,
                    ),
                    args_hash=args_hash,
                    risk_level=risk_level or classify_tool_risk(tool_name, kwargs),
                    input_summary=summarize_input(call_args),
                )
            )
            authz_decision = decision_payload.get("decision", {})
            decision = authz_decision.get("decision")
            if decision == "allow":
                return self._execute_authorized_tool(
                    tool_name=tool_name,
                    func=func,
                    call_args=args,
                    call_kwargs=kwargs,
                    args_hash=args_hash,
                    tool_call_id=decision_payload.get("tool_call_id"),
                    obligations=authz_decision.get("obligations"),
                    ui_hints=ui_hints,
                )
            if decision == "review":
                approval_id = decision_payload.get("approval_id")
                resume_decision = self._interrupt_for_approval(
                    approval_id=approval_id,
                    tool_call_id=decision_payload.get("tool_call_id"),
                    tool_name=tool_name,
                    args_hash=args_hash,
                    call_args=call_args,
                )
                if approval_granted(resume_decision):
                    self._raise_if_cancelled()
                    return self._execute_authorized_tool(
                        tool_name=tool_name,
                        func=func,
                        call_args=args,
                        call_kwargs=kwargs,
                        args_hash=args_hash,
                        tool_call_id=decision_payload.get("tool_call_id"),
                        obligations=authz_decision.get("obligations"),
                        ui_hints=ui_hints,
                    )
                raise ToolDenied(f"tool approval rejected: {tool_name}")
            raise ToolDenied(f"tool denied: {tool_name}")

        return wrapped

    def _execute_authorized_tool(
        self,
        *,
        tool_name: str,
        func: Callable[..., Any],
        call_args: tuple[Any, ...],
        call_kwargs: dict[str, Any],
        args_hash: str,
        tool_call_id: str | None,
        obligations: dict[str, Any] | None,
        ui_hints: dict[str, Any] | None,
    ) -> Any:
        context_path = (
            extract_path(call_kwargs)
            or extract_path(call_args)
            or extract_path({"args": call_args, "kwargs": call_kwargs})
        )
        stream_context = (
            current_file_tool_call(context_path, tool_name) if context_path else None
        )
        ui_tool_call_id = (
            str(stream_context["tool_call_id"])
            if stream_context and stream_context.get("tool_call_id")
            else None
        )
        self._raise_if_cancelled()
        try:
            with file_tool_call_context(
                tool_call_id=ui_tool_call_id or tool_call_id,
                tool_name=tool_name,
                path=context_path,
                operation=tool_name,
                args_hash=args_hash,
            ):
                self._raise_if_cancelled()
                result = func(*call_args, **call_kwargs)
                self._raise_if_cancelled()
            governed = apply_output_policy(result, obligations)
            self._raise_if_cancelled()
        except RunCancelled:
            raise
        except Exception as exc:
            try:
                self._emit_tool_event(
                    event_type="tool.call.failed",
                    tool_name=tool_name,
                    tool_call_id=tool_call_id,
                    args_hash=args_hash,
                    payload={
                        "status": "failed",
                        "error_type": exc.__class__.__name__,
                        "error_summary": summarize_input({"error": str(exc)}),
                        **(
                            {"ui_tool_call_id": ui_tool_call_id}
                            if ui_tool_call_id
                            else {}
                        ),
                    },
                )
            except Exception:
                pass
            raise

        if is_retryable_browser_failure(governed):
            error = governed.get("error")
            error_code = error.get("code") if isinstance(error, Mapping) else None
            error_message = error.get("message") if isinstance(error, Mapping) else None
            recovery = governed.get("recovery")
            event_payload = {
                "status": "failed",
                "retryable": True,
                "error_type": str(error_code or "BROWSER_RECOVERABLE_ERROR"),
                "error_summary": summarize_input(
                    {"error": str(error_message or "recoverable browser failure")}
                ),
            }
            if isinstance(recovery, Mapping):
                attempt = recovery.get("attempt")
                fingerprint = recovery.get("failure_fingerprint")
                if isinstance(attempt, int) and not isinstance(attempt, bool):
                    event_payload["recovery_attempt"] = attempt
                if isinstance(fingerprint, str):
                    event_payload["failure_fingerprint"] = fingerprint[:64]
            recovery_snapshot = governed.get("recovery_snapshot")
            browser = browser_event_projection(tool_name, recovery_snapshot)
            if browser:
                event_payload["browser"] = browser
            if ui_tool_call_id:
                event_payload["ui_tool_call_id"] = ui_tool_call_id
            self._emit_tool_event(
                event_type="tool.call.failed",
                tool_name=tool_name,
                tool_call_id=tool_call_id,
                args_hash=args_hash,
                payload=event_payload,
            )
            return governed

        event_payload = {
            "status": "completed",
            "output_summary": summarize_output(governed, obligations),
        }
        browser = browser_event_projection(tool_name, governed)
        if browser:
            event_payload["browser"] = browser
        if ui_tool_call_id:
            event_payload["ui_tool_call_id"] = ui_tool_call_id
        views = build_tool_result_views(
            governed,
            artifact_writer=self._artifact_writer(tool_call_id or args_hash),
            ui_hints=ui_hints,
        )
        self._raise_if_cancelled()
        if views:
            event_payload["views"] = views

        self._emit_tool_event(
            event_type="tool.call.completed",
            tool_name=tool_name,
            tool_call_id=tool_call_id,
            args_hash=args_hash,
            payload=event_payload,
        )
        return governed

    def _artifact_writer(
        self, event_key: str | None
    ) -> Callable[..., dict[str, Any] | None] | None:
        if not self.project_id:
            return None
        if not self.actor.user_id:
            return None

        def write_artifact(
            *, content: str, content_type: str, suffix: str
        ) -> dict[str, Any] | None:
            self._raise_if_cancelled()
            content_bytes = content.encode("utf-8")
            content_hash = hashlib.sha256(content_bytes).hexdigest()
            safe_key = (event_key or str(uuid4())).replace("/", "_")
            safe_suffix = suffix.replace("/", "_")
            path = f"/artifacts/tool-results/{safe_key}-{safe_suffix}"
            response = self.rust.file_write(
                {
                    "tenant_id": self.tenant_id,
                    "actor_user_id": str(self.actor.user_id),
                    "actor_device_id": str(self.actor.device_id)
                    if self.actor.device_id
                    else None,
                    "actor_session_id": str(self.actor.session_id)
                    if self.actor.session_id
                    else None,
                    "project_id": self.project_id,
                    "path": path,
                    "inline_content": content,
                    "content_type": content_type,
                    "expected_revision": 0,
                    "reason": "tool result artifact",
                    "run_id": self.run_id,
                }
            )
            artifact_ref = {
                "artifact_id": str(response.get("id") or path),
                "content_type": str(response.get("content_type") or content_type),
                "content_hash": normalize_hash(
                    response.get("content_hash"), content_hash
                ),
                "size_bytes": int(response.get("size_bytes") or len(content_bytes)),
            }
            object_reference_id = response.get("object_reference_id")
            if object_reference_id:
                artifact_ref["object_reference_id"] = str(object_reference_id)
            return artifact_ref

        return write_artifact

    def _raise_if_cancelled(self) -> None:
        if self.run_id and is_run_cancelled(str(self.run_id)):
            raise RunCancelled(str(self.run_id))

    def _interrupt_for_approval(
        self,
        *,
        approval_id: str | None,
        tool_call_id: str | None,
        tool_name: str,
        args_hash: str,
        call_args: dict[str, Any],
    ) -> Any:
        request = {
            "type": "platform_tool_approval",
            "approval_id": approval_id,
            "tool_call_id": tool_call_id,
            "tool_name": tool_name,
            "args_hash": args_hash,
            "input_summary": summarize_input(call_args),
            "decisions": [{"type": "approve"}, {"type": "reject"}],
        }
        try:
            return interrupt(request)
        except RuntimeError as exc:
            if "outside of a runnable context" in str(exc):
                raise ToolRequiresApproval(approval_id) from exc
            raise

    def _emit_tool_event(
        self,
        *,
        event_type: str,
        tool_name: str,
        tool_call_id: str | None,
        args_hash: str,
        payload: dict[str, Any],
    ) -> None:
        if not self.conversation_id:
            return
        event_key = tool_call_id or args_hash
        self.rust.emit_events(
            tenant_id=self.tenant_id,
            conversation_id=self.conversation_id,
            run_id=self.run_id,
            events=[
                {
                    "event_id": f"{event_type}.{event_key}",
                    "type": event_type,
                    "payload": {
                        "run_id": self.run_id,
                        "tool_call_id": tool_call_id,
                        "tool_name": tool_name,
                        "args_hash": args_hash,
                        **payload,
                    },
                    "trace_id": self.trace_id,
                }
            ],
        )


def browser_event_projection(
    tool_name: str, result: Any
) -> dict[str, str | int | bool] | None:
    if not tool_name.startswith("browser_") or not isinstance(result, Mapping):
        return None
    if result.get("kind") != "browser":
        return None

    projection: dict[str, str | int | bool] = {"kind": "browser"}
    string_limits = {
        "action": 64,
        "session_id": 128,
        "profile": 64,
        "url": 4096,
        "title": 512,
        "tab_id": 128,
        "auth_state": 64,
        "recovery_action": 64,
    }
    for key, limit in string_limits.items():
        value = result.get(key)
        if isinstance(value, str):
            projection[key] = value[:limit]
    element_count = result.get("element_count")
    if isinstance(element_count, int) and not isinstance(element_count, bool):
        projection["element_count"] = max(0, element_count)
    tab_count = result.get("tab_count")
    if isinstance(tab_count, int) and not isinstance(tab_count, bool):
        projection["tab_count"] = max(0, tab_count)
    closed = result.get("closed")
    if isinstance(closed, bool):
        projection["closed"] = closed
    for key in ("auth_expired", "recovered"):
        value = result.get(key)
        if isinstance(value, bool):
            projection[key] = value
    return projection


def is_retryable_browser_failure(result: Any) -> bool:
    return (
        isinstance(result, Mapping)
        and result.get("kind") == "browser"
        and result.get("status") == "failed"
        and result.get("retryable") is True
    )


def stable_hash(value: Any) -> str:
    payload = json.dumps(value, ensure_ascii=True, sort_keys=True, default=str)
    return hashlib.sha256(payload.encode("utf-8")).hexdigest()


def tool_authz_resource(
    tool_name: str,
    kwargs: dict[str, Any],
    *,
    static_resource: dict[str, Any] | None,
    actor: ActorRef,
) -> dict[str, Any] | None:
    if static_resource:
        return static_resource

    lowered = tool_name.lower()
    if lowered.startswith("browser_"):
        resource_id = actor.device_id or "unbound-device"
        return {"type": "local_exec", "id": str(resource_id)}
    if lowered in {"local_exec", "local_command", "run_local_command"} or any(
        marker in lowered for marker in ["local_exec", "local_command", "shell"]
    ):
        resource_id = kwargs.get("device_id") or actor.device_id or "unbound-device"
        return {"type": "local_exec", "id": str(resource_id)}

    if "sql" in lowered:
        if kwargs.get("sql_tool_id"):
            return {"type": "sql_tool", "id": str(kwargs["sql_tool_id"])}
        if kwargs.get("query_hash"):
            return {"type": "sql_query", "id": str(kwargs["query_hash"])}
        return {"type": "sql_query", "id": tool_name}

    if lowered.startswith(("mcp.", "mcp_")) or "mcp" in lowered:
        if kwargs.get("mcp_tool_id"):
            return {"type": "mcp_tool", "id": str(kwargs["mcp_tool_id"])}
        if kwargs.get("server_id"):
            return {"type": "mcp_server", "id": str(kwargs["server_id"])}

    if lowered.startswith(("third_party", "http_tool", "external_tool", "tool_")):
        if kwargs.get("tool_id"):
            return {"type": "tool", "id": str(kwargs["tool_id"])}
        if kwargs.get("tool_version_id"):
            return {"type": "tool_version", "id": str(kwargs["tool_version_id"])}

    return None


def normalize_hash(value: Any, fallback: str) -> str:
    text = str(value or fallback)
    return text if text.startswith("sha256:") else f"sha256:{text}"


def approval_granted(value: Any) -> bool:
    if not isinstance(value, dict):
        return False
    decision_payload = value.get("decision_payload")
    if isinstance(decision_payload, dict):
        decision = decision_payload.get("decision")
        if decision in {"approved", "approve", "allow"}:
            return True
    decision = value.get("decision")
    if decision in {"approved", "approve", "allow"}:
        return True
    decisions = value.get("decisions")
    if isinstance(decisions, list) and decisions:
        first = decisions[0]
        return isinstance(first, dict) and first.get("type") == "approve"
    return False
