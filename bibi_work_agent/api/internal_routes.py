from __future__ import annotations

from uuid import UUID

from fastapi import APIRouter, Depends, Header, HTTPException, status

from bibi_work_agent.api.schemas import (
    CancelRunRequest,
    ResumeRunRequest,
    RunDispatchRequest,
)
from bibi_work_agent.runtime.cancellation import mark_run_cancelled
from bibi_work_agent.settings import settings
from bibi_work_agent.workers.tasks import execute_run, resume_agent_run


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


@router.post("/internal/agent-runs", dependencies=[Depends(require_internal_token)])
def dispatch_run(payload: RunDispatchRequest) -> dict[str, str]:
    task = execute_run.delay(payload.model_dump(mode="json"))
    return {"status": "queued", "task_id": task.id}


@router.post(
    "/internal/agent-runs/{run_id}/resume",
    dependencies=[Depends(require_internal_token)],
)
def resume_run(run_id: UUID, payload: ResumeRunRequest) -> dict[str, str]:
    task = resume_agent_run.delay(str(run_id), payload.model_dump(mode="json"))
    return {"status": "queued", "task_id": task.id}


@router.post(
    "/internal/agent-runs/{run_id}/cancel",
    dependencies=[Depends(require_internal_token)],
)
def cancel_run(run_id: UUID, payload: CancelRunRequest) -> dict[str, str]:
    marked = mark_run_cancelled(str(run_id))
    return {
        "status": "cancel_marked" if marked else "cancel_mark_failed",
        "run_id": str(run_id),
        "reason": payload.reason,
    }
