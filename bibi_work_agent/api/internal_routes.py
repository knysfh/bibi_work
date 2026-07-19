from __future__ import annotations

from uuid import UUID

from fastapi import APIRouter, Depends, Header, HTTPException, Response, status
from redis.exceptions import RedisError

from bibi_work_agent.api.schemas import (
    CancelRunRequest,
    ResumeRunRequest,
    RunDispatchRequest,
)
from bibi_work_agent.runtime.cancellation import (
    active_task_id,
    clear_active_task,
    mark_run_cancelled,
)
from bibi_work_agent.settings import settings
from bibi_work_agent.telemetry import current_trace_headers
from bibi_work_agent.workers.celery_app import celery_app
from bibi_work_agent.workers.tasks import execute_run, resume_agent_run
from bibi_work_agent.workers.metrics import render_prometheus, snapshot


router = APIRouter()


def require_internal_token(authorization: str | None = Header(default=None)) -> None:
    expected = f"Bearer {settings.internal_token}"
    if not settings.internal_token or authorization != expected:
        raise HTTPException(
            status_code=status.HTTP_401_UNAUTHORIZED,
            detail="invalid internal bearer token",
        )


@router.get("/health")
def health() -> dict[str, str]:
    return {"status": "ok"}


@router.get("/internal/metrics", dependencies=[Depends(require_internal_token)])
def operational_metrics() -> Response:
    try:
        body = render_prometheus(snapshot())
    except RedisError as error:
        raise HTTPException(
            status_code=status.HTTP_503_SERVICE_UNAVAILABLE,
            detail="Celery metrics are unavailable",
        ) from error
    return Response(body, media_type="text/plain; version=0.0.4")


@router.post("/internal/agent-runs", dependencies=[Depends(require_internal_token)])
def dispatch_run(payload: RunDispatchRequest) -> dict[str, str]:
    task = execute_run.apply_async(
        args=[payload.model_dump(mode="json")], headers=current_trace_headers()
    )
    return {"status": "queued", "task_id": task.id}


@router.post(
    "/internal/agent-runs/{run_id}/resume",
    dependencies=[Depends(require_internal_token)],
)
def resume_run(run_id: UUID, payload: ResumeRunRequest) -> dict[str, str]:
    task = resume_agent_run.apply_async(
        args=[str(run_id), payload.model_dump(mode="json")],
        headers=current_trace_headers(),
    )
    return {"status": "queued", "task_id": task.id}


@router.post(
    "/internal/agent-runs/{run_id}/cancel",
    dependencies=[Depends(require_internal_token)],
)
def cancel_run(
    run_id: UUID, payload: CancelRunRequest
) -> dict[str, str | bool | None]:
    marked = mark_run_cancelled(str(run_id))
    task_id = active_task_id(str(run_id))
    termination_requested = False
    if task_id:
        celery_app.control.revoke(task_id, terminate=True, signal="SIGKILL")
        clear_active_task(str(run_id), task_id)
        termination_requested = True
    return {
        "status": "cancel_marked" if marked else "cancel_mark_failed",
        "run_id": str(run_id),
        "reason": payload.reason,
        "task_id": task_id,
        "termination_requested": termination_requested,
    }
