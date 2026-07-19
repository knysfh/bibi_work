from __future__ import annotations

from celery import Celery

from bibi_work_agent.settings import settings
from bibi_work_agent.telemetry import configure_telemetry


configure_telemetry()
celery_app = Celery(
    "bibi_work_agent",
    broker=settings.celery_broker_url,
    backend=settings.celery_result_backend,
    include=["bibi_work_agent.workers.tasks"],
)
celery_app.conf.update(
    task_acks_late=True,
    task_reject_on_worker_lost=True,
    worker_prefetch_multiplier=1,
)
