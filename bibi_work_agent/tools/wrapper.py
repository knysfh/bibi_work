from __future__ import annotations

import hashlib
import json
from uuid import uuid4
from collections.abc import Callable
from functools import wraps
from typing import Any

from langgraph.types import interrupt

from bibi_work_agent.api.schemas import ActorRef, ToolAuthorizeRequest
from bibi_work_agent.clients.rust_client import RustClient
from bibi_work_agent.tools.io_policy import (
    apply_output_policy,
    summarize_input,
    summarize_output,
)
from bibi_work_agent.tools.result_presenter import build_tool_result_views
from bibi_work_agent.tools.risk import classify_tool_risk


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
    ) -> Callable[..., Any]:
        @wraps(func)
        def wrapped(*args: Any, **kwargs: Any) -> Any:
            call_args = {"args": args, "kwargs": kwargs}
            args_hash = stable_hash(call_args)
            decision_payload = self.rust.authorize_tool(
                ToolAuthorizeRequest(
                    tenant_id=self.tenant_id,
                    actor=self.actor,
                    conversation_id=self.conversation_id,
                    run_id=self.run_id,
                    tool_name=tool_name,
                    args_hash=args_hash,
                    risk_level=classify_tool_risk(tool_name, kwargs),
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
        try:
            result = func(*call_args, **call_kwargs)
            governed = apply_output_policy(result, obligations)
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
                    },
                )
            except Exception:
                pass
            raise

        event_payload = {
            "status": "completed",
            "output_summary": summarize_output(governed, obligations),
        }
        views = build_tool_result_views(
            governed,
            artifact_writer=self._artifact_writer(tool_call_id or args_hash),
            ui_hints=ui_hints,
        )
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

    def _artifact_writer(self, event_key: str | None) -> Callable[..., dict[str, Any] | None] | None:
        if not self.project_id:
            return None
        if not self.actor.user_id:
            return None

        def write_artifact(*, content: str, content_type: str, suffix: str) -> dict[str, Any] | None:
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
                "content_hash": normalize_hash(response.get("content_hash"), content_hash),
                "size_bytes": int(response.get("size_bytes") or len(content_bytes)),
            }
            object_reference_id = response.get("object_reference_id")
            if object_reference_id:
                artifact_ref["object_reference_id"] = str(object_reference_id)
            return artifact_ref

        return write_artifact

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


def stable_hash(value: Any) -> str:
    payload = json.dumps(value, ensure_ascii=True, sort_keys=True, default=str)
    return hashlib.sha256(payload.encode("utf-8")).hexdigest()


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
