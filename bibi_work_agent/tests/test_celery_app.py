from __future__ import annotations

from bibi_work_agent.workers.celery_app import celery_app


def test_celery_worker_imports_runtime_tasks() -> None:
    celery_app.loader.import_default_modules()

    assert "bibi_work_agent.execute_run" in celery_app.tasks
    assert "bibi_work_agent.resume_run" in celery_app.tasks
