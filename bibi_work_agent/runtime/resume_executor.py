from __future__ import annotations

import time
from typing import Any
from uuid import uuid4

from langgraph.types import Command

from bibi_work_agent.clients.rust_client import RustClient
from bibi_work_agent.runtime.agent_factory import create_platform_agent
from bibi_work_agent.runtime.cancellation import RunCancelled, is_run_cancelled
from bibi_work_agent.runtime.event_emitter import EventEmitter
from bibi_work_agent.runtime.event_normalizer import AgentEventNormalizer
from bibi_work_agent.runtime.error_reporting import run_failure_payload
from bibi_work_agent.runtime.memory_candidates import MemoryCandidateCollector
from bibi_work_agent.runtime.resume_idempotency import ResumeIdempotencyStore
from bibi_work_agent.runtime.snapshot_contract import (
    safe_error_message,
    validate_run_config_snapshot,
)
from bibi_work_agent.runtime.run_executor import (
    WAITING_EVENT_TYPES,
    call_agent_invoke,
    call_agent_stream,
    emit_approval_requested,
    emit_cancelled,
    graph_config_for_payload,
    message_context_from_snapshot,
    prepare_agent_payload,
    capture_completion_results,
)
from bibi_work_agent.tools.wrapper import ToolRequiresApproval


def resume_run_payload(
    worker_task_id: str | None, run_id: str, payload: dict[str, Any]
) -> None:
    payload = prepare_agent_payload({"run_id": run_id, **payload})
    conversation_id = payload.get("conversation_id")
    if not conversation_id:
        raise RuntimeError("conversation_id is required for approval resume")

    rust = RustClient()
    emitter = EventEmitter(
        rust=rust,
        tenant_id=payload["tenant_id"],
        conversation_id=conversation_id,
        run_id=run_id,
    )
    approval_id = str(payload["approval_id"])
    idempotency = ResumeIdempotencyStore()
    lease = idempotency.acquire(approval_id)
    if not lease.acquired:
        return

    started = int(time.time() * 1000)
    try:
        if is_run_cancelled(str(run_id)):
            emit_cancelled(
                run_id=run_id,
                trace_id=payload.get("trace_id"),
                emitter=emitter,
                reason="cancelled_before_resume",
            )
            idempotency.mark_completed(approval_id)
            return

        validate_run_config_snapshot(payload.get("run_config_snapshot"))

        emitter.emit(
            [
                {
                    "event_id": f"run.resumed.{run_id}.{approval_id}",
                    "type": "run.started",
                    "payload": {
                        "run_id": run_id,
                        "approval_id": approval_id,
                        "worker_task_id": worker_task_id,
                        "status": "running",
                    },
                    "trace_id": payload.get("trace_id"),
                }
            ]
        )

        memory_candidates = MemoryCandidateCollector()
        completion_results: list[Any] = []
        waiting_or_cancelled = resume_deepagent(
            payload,
            emitter,
            memory_candidates=memory_candidates,
            completion_results=completion_results,
        )
    except RunCancelled as exc:
        emit_cancelled(
            run_id=run_id,
            trace_id=payload.get("trace_id"),
            emitter=emitter,
            reason=exc.reason,
        )
        idempotency.mark_completed(approval_id)
        return
    except ToolRequiresApproval as exc:
        emit_approval_requested(
            run_id=run_id,
            trace_id=payload.get("trace_id"),
            emitter=emitter,
            approval_id=exc.approval_id,
            worker_task_id=worker_task_id,
        )
        idempotency.mark_completed(approval_id)
        return
    except Exception as exc:  # noqa: BLE001
        error = safe_error_message(exc)
        failure = run_failure_payload(exc)
        emitter.emit(
            [
                {
                    "event_id": f"run.resume.failed.{run_id}.{approval_id}.{uuid4()}",
                    "type": "run.failed",
                    "payload": {
                        "run_id": run_id,
                        "approval_id": approval_id,
                        "worker_task_id": worker_task_id,
                        "error_type": exc.__class__.__name__,
                        **failure,
                    },
                    "trace_id": payload.get("trace_id"),
                }
            ]
        )
        idempotency.mark_failed(approval_id)
        raise RuntimeError(error) from None

    idempotency.mark_completed(approval_id)
    if waiting_or_cancelled:
        return

    completion_payload = {
        "run_id": run_id,
        "approval_id": approval_id,
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
                "event_id": f"run.completed.{run_id}.{approval_id}",
                "type": "run.completed",
                "payload": completion_payload,
                "trace_id": payload.get("trace_id"),
            }
        ]
    )


def resume_deepagent(
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
    normalizer = AgentEventNormalizer(
        run_id=run_id,
        trace_id=trace_id,
        message_context=message_context_from_snapshot(snapshot),
    )
    graph_config = graph_config_for_payload(payload)
    command = Command(resume=resume_value_for_payload(payload))

    emitted_any = False
    if hasattr(agent, "stream"):
        for raw_event in call_agent_stream(agent, command, graph_config):
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
    else:
        result = call_agent_invoke(agent, command, graph_config)
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


def resume_value_for_payload(payload: dict[str, Any]) -> dict[str, Any]:
    decision_payload = payload.get("decision_payload") or {}
    decision = decision_payload.get("decision")
    if decision in {"approved", "approve", "allow"}:
        decisions = [{"type": "approve"}]
    elif decision in {"rejected", "reject", "deny"}:
        decisions = [
            {
                "type": "reject",
                "message": decision_payload.get("reason") or "approval rejected",
            }
        ]
    else:
        decisions = decision_payload.get("decisions") or []
    return {
        "approval_id": payload["approval_id"],
        "decision": decision,
        "decision_payload": decision_payload,
        "decisions": decisions,
    }
