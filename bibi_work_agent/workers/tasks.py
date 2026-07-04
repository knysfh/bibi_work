from __future__ import annotations

from typing import Any

import httpx

from bibi_work_agent.runtime.resume_executor import resume_run_payload
from bibi_work_agent.runtime.run_executor import execute_run_payload
from bibi_work_agent.workers.celery_app import celery_app


@celery_app.task(
    name="bibi_work_agent.execute_run",
    bind=True,
    autoretry_for=(httpx.HTTPError,),
    retry_backoff=True,
    max_retries=3,
)
def execute_run(self: Any, payload: dict[str, Any]) -> None:
    execute_run_payload(self.request.id, payload)


@celery_app.task(
    name="bibi_work_agent.resume_run",
    bind=True,
    autoretry_for=(httpx.HTTPError,),
    retry_backoff=True,
    max_retries=3,
)
def resume_agent_run(self: Any, run_id: str, payload: dict[str, Any]) -> None:
    resume_run_payload(self.request.id, run_id, payload)
