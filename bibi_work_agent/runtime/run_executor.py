from __future__ import annotations

import inspect
import json
import time
from collections.abc import Callable, Iterable
from typing import Any
from uuid import uuid4

from bibi_work_agent.clients.rust_client import RustClient
from bibi_work_agent.runtime.agent_factory import create_platform_agent
from bibi_work_agent.runtime.cancellation import RunCancelled, is_run_cancelled
from bibi_work_agent.runtime.event_emitter import EventEmitter
from bibi_work_agent.runtime.event_normalizer import AgentEventNormalizer
from bibi_work_agent.runtime.memory_candidates import MemoryCandidateCollector
from bibi_work_agent.runtime.snapshot_contract import (
    safe_error_message,
    validate_run_config_snapshot,
)
from bibi_work_agent.tools.wrapper import ToolRequiresApproval


WAITING_EVENT_TYPES = {"interrupt.requested", "approval.requested"}


def execute_run_payload(worker_task_id: str | None, payload: dict[str, Any]) -> None:
    payload = prepare_agent_payload(payload)
    tenant_id = required_dispatch_field(payload, "tenant_id")
    conversation_id = required_dispatch_field(payload, "conversation_id")
    run_id = required_dispatch_field(payload, "run_id")
    trace_id = payload.get("trace_id")
    started = int(time.time() * 1000)
    rust = RustClient()
    emitter = EventEmitter(
        rust=rust,
        tenant_id=tenant_id,
        conversation_id=conversation_id,
        run_id=run_id,
    )

    if is_run_cancelled(str(run_id)):
        emit_cancelled(
            run_id=run_id,
            trace_id=trace_id,
            emitter=emitter,
            reason="cancelled_before_start",
        )
        return

    try:
        validate_run_config_snapshot(payload.get("run_config_snapshot"))
    except Exception as exc:  # noqa: BLE001
        error = safe_error_message(exc)
        emitter.emit(
            [
                {
                    "event_id": f"run.failed.{run_id}.{uuid4()}",
                    "type": "run.failed",
                    "payload": {"run_id": run_id, "error": error},
                    "trace_id": trace_id,
                }
            ]
        )
        raise RuntimeError(error) from None

    emitter.emit(
        [
            {
                "event_id": f"run.started.{run_id}",
                "type": "run.started",
                "payload": {"run_id": run_id, "worker_task_id": worker_task_id},
                "trace_id": trace_id,
            }
        ]
    )

    try:
        memory_candidates = MemoryCandidateCollector()
        completion_results: list[Any] = []
        cancelled = run_deepagent(
            payload,
            emitter,
            memory_candidates=memory_candidates,
            completion_results=completion_results,
        )
    except RunCancelled as exc:
        emit_cancelled(
            run_id=run_id,
            trace_id=trace_id,
            emitter=emitter,
            reason=exc.reason,
        )
        return
    except ToolRequiresApproval as exc:
        emit_approval_requested(
            run_id=run_id,
            trace_id=trace_id,
            emitter=emitter,
            approval_id=exc.approval_id,
            worker_task_id=worker_task_id,
        )
        return
    except Exception as exc:  # noqa: BLE001 - worker must report failures to Rust.
        error = safe_error_message(exc)
        emitter.emit(
            [
                {
                    "event_id": f"run.failed.{run_id}.{uuid4()}",
                    "type": "run.failed",
                    "payload": {"run_id": run_id, "error": error},
                    "trace_id": trace_id,
                }
            ]
        )
        raise RuntimeError(error) from None

    if cancelled:
        return

    completion_payload = {
        "run_id": run_id,
        "duration_ms": int(time.time() * 1000) - started,
    }
    if completion_results:
        completion_payload["result"] = completion_results[-1]
    candidates = memory_candidates.candidates()
    if candidates:
        completion_payload["memory_candidates"] = candidates

    emitter.emit(
        [
            {
                "event_id": f"run.completed.{run_id}",
                "type": "run.completed",
                "payload": completion_payload,
                "trace_id": trace_id,
            }
        ]
    )


def run_deepagent(
    payload: dict[str, Any],
    emitter: EventEmitter,
    *,
    memory_candidates: MemoryCandidateCollector | None = None,
    completion_results: list[Any] | None = None,
) -> bool:
    payload = prepare_agent_payload(payload)
    run_id = payload["run_id"]
    trace_id = payload.get("trace_id")
    if is_run_cancelled(str(run_id)):
        emit_cancelled(
            run_id=run_id, trace_id=trace_id, emitter=emitter, reason="cancelled"
        )
        return True

    snapshot = payload.get("run_config_snapshot", {})
    agent = create_platform_agent(snapshot)
    input_payload = agent_input_payload(payload.get("input", {}), snapshot)
    graph_config = graph_config_for_payload(payload)
    normalizer = AgentEventNormalizer(
        run_id=run_id,
        trace_id=trace_id,
        message_context=message_context_from_snapshot(snapshot),
    )

    emitted_any = False
    if hasattr(agent, "stream"):
        for raw_event in call_agent_stream(agent, input_payload, graph_config):
            if memory_candidates is not None:
                memory_candidates.observe(raw_event)
            if is_run_cancelled(str(run_id)):
                emit_cancelled(
                    run_id=run_id,
                    trace_id=trace_id,
                    emitter=emitter,
                    reason="cancelled",
                )
                return True
            events = normalizer.normalize(raw_event)
            capture_completion_results(events, completion_results)
            if memory_candidates is not None:
                for event in events:
                    memory_candidates.observe(event)
            if events:
                emitted_any = True
                emitter.emit(events)
                if any(event.get("type") in WAITING_EVENT_TYPES for event in events):
                    return True
            if is_run_cancelled(str(run_id)):
                emit_cancelled(
                    run_id=run_id,
                    trace_id=trace_id,
                    emitter=emitter,
                    reason="cancelled",
                )
                return True
    else:
        result = call_agent_invoke(agent, input_payload, graph_config)
        if memory_candidates is not None:
            memory_candidates.observe(result)
        if is_run_cancelled(str(run_id)):
            emit_cancelled(
                run_id=run_id, trace_id=trace_id, emitter=emitter, reason="cancelled"
            )
            return True
        emitted_any = True
        completed_event = normalizer.completed_message(result)
        capture_completion_results([completed_event], completion_results)
        if memory_candidates is not None:
            memory_candidates.observe(completed_event)
        emitter.emit([completed_event])

    completed_event = normalizer.pending_completed_message()
    if completed_event is not None:
        emitted_any = True
        capture_completion_results([completed_event], completion_results)
        if memory_candidates is not None:
            memory_candidates.observe(completed_event)
        emitter.emit([completed_event])

    if not emitted_any:
        completed_event = (
            normalizer.platform_tool_result_completed_message()
            or normalizer.completed_message(
                {"message": "Run completed without an assistant response."}
            )
        )
    if not emitted_any and completed_event is not None:
        capture_completion_results([completed_event], completion_results)
        emitter.emit([completed_event])

    return False


def agent_input_payload(input_payload: Any, snapshot: dict[str, Any]) -> Any:
    workflow = snapshot.get("workflow")
    if not isinstance(workflow, dict) or not isinstance(input_payload, dict):
        return input_payload
    if isinstance(input_payload.get("messages"), list):
        return input_payload

    node = input_payload.get("node")
    instruction = node.get("instruction") if isinstance(node, dict) else None
    context = input_payload.get("node_input")
    if context is None:
        context = {
            "workflow_input": input_payload.get("workflow_input"),
            "upstream_outputs": input_payload.get("upstream_outputs", {}),
        }
    content_parts = []
    if isinstance(instruction, str) and instruction.strip():
        content_parts.append(instruction.strip())
    content_parts.append(
        "Workflow node input:\n"
        + json.dumps(context, ensure_ascii=False, separators=(",", ":"), default=str)
    )
    return {
        **input_payload,
        "messages": [{"role": "user", "content": "\n\n".join(content_parts)}],
    }


def capture_completion_results(
    events: Iterable[dict[str, Any]], result_sink: list[Any] | None
) -> None:
    if result_sink is None:
        return
    for event in events:
        if event.get("type") != "message.completed":
            continue
        payload = event.get("payload")
        if isinstance(payload, dict) and "result" in payload:
            result_sink.append(payload["result"])


def prepare_agent_payload(payload: dict[str, Any]) -> dict[str, Any]:
    prepared = dict(payload)
    snapshot = dict(prepared.get("run_config_snapshot") or {})
    for key in ("tenant_id", "conversation_id", "run_id", "trace_id", "thread_id"):
        value = prepared.get(key)
        if value is not None:
            snapshot[key] = value
        elif snapshot.get(key) is not None:
            prepared[key] = snapshot[key]

    thread_id = (
        prepared.get("thread_id") or snapshot.get("thread_id") or prepared.get("run_id")
    )
    if thread_id is not None:
        prepared["thread_id"] = str(thread_id)
        snapshot["thread_id"] = str(thread_id)
    prepared["run_config_snapshot"] = snapshot
    return prepared


def message_context_from_snapshot(snapshot: dict[str, Any]) -> dict[str, Any]:
    cron = snapshot.get("cron")
    if not isinstance(cron, dict):
        return {}
    context: dict[str, Any] = {}
    job_id = cron.get("job_id") or cron.get("cron_job_id")
    if job_id:
        context["cron_job_id"] = str(job_id)
    job_name = cron.get("job_name") or cron.get("cron_job_name")
    if job_name:
        context["cron_job_name"] = str(job_name)
    return context


def required_dispatch_field(payload: dict[str, Any], key: str) -> str:
    value = payload.get(key)
    if value is None or (isinstance(value, str) and not value.strip()):
        raise RuntimeError(f"{key} is required for agent run dispatch")
    return str(value)


def graph_config_for_payload(payload: dict[str, Any]) -> dict[str, Any]:
    snapshot = payload.get("run_config_snapshot") or {}
    thread_id = (
        payload.get("thread_id") or snapshot.get("thread_id") or payload["run_id"]
    )
    configurable: dict[str, Any] = {"thread_id": str(thread_id)}
    checkpoint_ns = payload.get("checkpoint_ns") or snapshot.get("checkpoint_ns")
    if checkpoint_ns is not None:
        configurable["checkpoint_ns"] = str(checkpoint_ns)
    checkpoint_id = payload.get("checkpoint_id") or snapshot.get("checkpoint_id")
    if checkpoint_id is not None:
        configurable["checkpoint_id"] = str(checkpoint_id)
    return {"configurable": configurable}


def call_agent_stream(
    agent: Any,
    input_payload: Any,
    graph_config: dict[str, Any],
) -> Iterable[Any]:
    stream = agent.stream
    kwargs: dict[str, Any] = {}
    if callable_accepts_parameter(stream, "config"):
        kwargs["config"] = graph_config
    if callable_accepts_parameter(stream, "stream_mode"):
        kwargs["stream_mode"] = ["messages", "values"]
    return stream(input_payload, **kwargs)


def call_agent_invoke(
    agent: Any, input_payload: Any, graph_config: dict[str, Any]
) -> Any:
    invoke = agent.invoke
    if callable_accepts_config(invoke):
        return invoke(input_payload, config=graph_config)
    return invoke(input_payload)


def callable_accepts_config(func: Callable[..., Any]) -> bool:
    return callable_accepts_parameter(func, "config")


def callable_accepts_parameter(func: Callable[..., Any], name: str) -> bool:
    try:
        signature = inspect.signature(func)
    except (TypeError, ValueError):
        return True
    return name in signature.parameters or any(
        parameter.kind == inspect.Parameter.VAR_KEYWORD
        for parameter in signature.parameters.values()
    )


def emit_cancelled(
    *,
    run_id: str,
    trace_id: str | None,
    emitter: EventEmitter,
    reason: str,
) -> None:
    emitter.emit(
        [
            {
                "event_id": f"run.cancelled.runtime.{run_id}.{uuid4()}",
                "type": "run.cancelled",
                "payload": {"run_id": run_id, "reason": reason},
                "trace_id": trace_id,
            }
        ]
    )


def emit_approval_requested(
    *,
    run_id: str,
    trace_id: str | None,
    emitter: EventEmitter,
    approval_id: str | None,
    worker_task_id: str | None,
) -> None:
    emitter.emit(
        [
            {
                "event_id": f"approval.requested.runtime.{run_id}.{uuid4()}",
                "type": "approval.requested",
                "payload": {
                    "run_id": run_id,
                    "approval_id": approval_id,
                    "worker_task_id": worker_task_id,
                    "status": "waiting_approval",
                },
                "trace_id": trace_id,
            }
        ]
    )
