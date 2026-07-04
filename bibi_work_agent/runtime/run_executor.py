from __future__ import annotations

import inspect
import time
from collections.abc import Callable, Iterable
from typing import Any
from uuid import uuid4

from bibi_work_agent.clients.rust_client import RustClient
from bibi_work_agent.runtime.agent_factory import create_platform_agent
from bibi_work_agent.runtime.cancellation import is_run_cancelled
from bibi_work_agent.runtime.event_emitter import EventEmitter
from bibi_work_agent.runtime.event_normalizer import AgentEventNormalizer
from bibi_work_agent.runtime.memory_candidates import MemoryCandidateCollector
from bibi_work_agent.tools.wrapper import ToolRequiresApproval


WAITING_EVENT_TYPES = {"interrupt.requested", "approval.requested"}


def execute_run_payload(worker_task_id: str | None, payload: dict[str, Any]) -> None:
    tenant_id = payload["tenant_id"]
    conversation_id = payload["conversation_id"]
    run_id = payload["run_id"]
    trace_id = payload["trace_id"]
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
        cancelled = run_deepagent(payload, emitter, memory_candidates=memory_candidates)
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
        emitter.emit(
            [
                {
                    "event_id": f"run.failed.{run_id}.{uuid4()}",
                    "type": "run.failed",
                    "payload": {"run_id": run_id, "error": str(exc)},
                    "trace_id": trace_id,
                }
            ]
        )
        raise

    if cancelled:
        return

    completion_payload = {
        "run_id": run_id,
        "duration_ms": int(time.time() * 1000) - started,
    }
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
    input_payload = payload.get("input", {})
    graph_config = graph_config_for_payload(payload)
    normalizer = AgentEventNormalizer(
        run_id=run_id,
        trace_id=trace_id,
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
        if memory_candidates is not None:
            memory_candidates.observe(completed_event)
        emitter.emit([completed_event])

    if not emitted_any:
        emitter.emit(
            [
                normalizer.completed_message(
                    {"message": "agent finished without streamable platform events"}
                )
            ]
        )

    return False


def prepare_agent_payload(payload: dict[str, Any]) -> dict[str, Any]:
    prepared = dict(payload)
    snapshot = dict(prepared.get("run_config_snapshot") or {})
    for key in ("tenant_id", "conversation_id", "run_id", "trace_id", "thread_id"):
        value = prepared.get(key)
        if value is not None:
            snapshot[key] = value

    thread_id = (
        prepared.get("thread_id") or snapshot.get("thread_id") or prepared.get("run_id")
    )
    if thread_id is not None:
        prepared["thread_id"] = str(thread_id)
        snapshot["thread_id"] = str(thread_id)
    prepared["run_config_snapshot"] = snapshot
    return prepared


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
