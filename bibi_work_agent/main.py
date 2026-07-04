from __future__ import annotations

import uvicorn

from bibi_work_agent.api.app import app
from bibi_work_agent.settings import settings
from bibi_work_agent.workers.celery_app import celery_app


def run() -> None:
    uvicorn.run(app, host="0.0.0.0", port=settings.port)


__all__ = ["app", "celery_app", "run"]
