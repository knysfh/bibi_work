from __future__ import annotations

from bibi_work_agent.workers.metrics import render_prometheus, snapshot


class FakePipeline:
    def llen(self, _key: str) -> "FakePipeline":
        return self

    def hlen(self, _key: str) -> "FakePipeline":
        return self

    def mget(self, *_keys: str) -> "FakePipeline":
        return self

    def execute(self) -> list:
        return [7, 2, ["11", "9", "2"]]


class FakeRedis:
    def pipeline(self, *, transaction: bool) -> FakePipeline:
        assert transaction is False
        return FakePipeline()


def test_snapshot_and_prometheus_contract_are_low_cardinality() -> None:
    metrics = snapshot(FakeRedis())
    assert metrics.queue_depth == 7
    assert metrics.active_tasks == 2
    assert metrics.worker_task_failures_total == 2
    assert render_prometheus(metrics) == (
        "# TYPE bibi_celery_queue_depth gauge\n"
        "bibi_celery_queue_depth 7\n"
        "# TYPE bibi_celery_tasks_active gauge\n"
        "bibi_celery_tasks_active 2\n"
        "# TYPE bibi_celery_tasks_started_total counter\n"
        "bibi_celery_tasks_started_total 11\n"
        "# TYPE bibi_celery_tasks_succeeded_total counter\n"
        "bibi_celery_tasks_succeeded_total 9\n"
        "# TYPE bibi_celery_worker_task_failures_total counter\n"
        "bibi_celery_worker_task_failures_total 2\n"
    )
