from __future__ import annotations

from uuid import uuid4

from fastapi.testclient import TestClient

from bibi_work_agent.api import internal_routes
from bibi_work_agent.api.app import app
from bibi_work_agent.settings import Settings
from bibi_work_agent.workers.metrics import CeleryMetricsSnapshot


class FakeTask:
    id = "resume-task"


class FakeResumeTask:
    def __init__(self) -> None:
        self.calls: list[tuple[list, dict[str, str]]] = []

    def apply_async(self, *, args: list, headers: dict[str, str]):
        self.calls.append((args, headers))
        return FakeTask()


def test_metrics_endpoint_requires_internal_token_and_returns_prometheus(monkeypatch) -> None:
    monkeypatch.setattr(
        internal_routes, "settings", Settings(internal_token="test-token")
    )
    monkeypatch.setattr(
        internal_routes,
        "snapshot",
        lambda: CeleryMetricsSnapshot(3, 1, 10, 9, 1),
    )
    client = TestClient(app)

    denied = client.get("/internal/metrics")
    assert denied.status_code == 401

    response = client.get(
        "/internal/metrics", headers={"Authorization": "Bearer test-token"}
    )
    assert response.status_code == 200
    assert response.headers["content-type"].startswith("text/plain")
    assert "bibi_celery_queue_depth 3" in response.text
    assert "bibi_celery_worker_task_failures_total 1" in response.text


def test_resume_endpoint_queues_checkpoint_continuation(monkeypatch) -> None:
    monkeypatch.setattr(
        internal_routes, "settings", Settings(internal_token="test-token")
    )
    fake_resume = FakeResumeTask()
    monkeypatch.setattr(internal_routes, "resume_agent_run", fake_resume)
    client = TestClient(app)

    run_id = uuid4()
    response = client.post(
        f"/internal/agent-runs/{run_id}/resume",
        headers={
            "Authorization": "Bearer test-token",
            "traceparent": "00-0123456789abcdeffedcba9876543210-0123456789abcdef-01",
        },
        json={
            "tenant_id": str(uuid4()),
            "conversation_id": str(uuid4()),
            "approval_id": str(uuid4()),
            "decision_payload": {"decision": "approved"},
        },
    )

    assert response.status_code == 200
    assert response.json() == {"status": "queued", "task_id": "resume-task"}
    args, trace_headers = fake_resume.calls[0]
    assert args[0] == str(run_id)
    assert args[1]["decision_payload"] == {"decision": "approved"}
    assert trace_headers["traceparent"].split("-")[1] == (
        "0123456789abcdeffedcba9876543210"
    )


def test_cancel_endpoint_marks_and_terminates_active_celery_task(monkeypatch) -> None:
    class FakeControl:
        def __init__(self) -> None:
            self.revocations: list[tuple[str, bool, str]] = []

        def revoke(self, task_id: str, *, terminate: bool, signal: str) -> None:
            self.revocations.append((task_id, terminate, signal))

    class FakeCelery:
        def __init__(self) -> None:
            self.control = FakeControl()

    monkeypatch.setattr(
        internal_routes, "settings", Settings(internal_token="test-token")
    )
    monkeypatch.setattr(internal_routes, "mark_run_cancelled", lambda _run_id: True)
    monkeypatch.setattr(
        internal_routes, "active_task_id", lambda _run_id: "active-task-1"
    )
    cleared: list[tuple[str, str]] = []
    monkeypatch.setattr(
        internal_routes,
        "clear_active_task",
        lambda run_id, task_id: cleared.append((run_id, task_id)) or True,
    )
    fake_celery = FakeCelery()
    monkeypatch.setattr(internal_routes, "celery_app", fake_celery)
    client = TestClient(app)
    run_id = uuid4()

    response = client.post(
        f"/internal/agent-runs/{run_id}/cancel",
        headers={"Authorization": "Bearer test-token"},
        json={
            "tenant_id": str(uuid4()),
            "conversation_id": str(uuid4()),
            "reason": "user_cancelled",
        },
    )

    assert response.status_code == 200
    assert response.json() == {
        "status": "cancel_marked",
        "run_id": str(run_id),
        "reason": "user_cancelled",
        "task_id": "active-task-1",
        "termination_requested": True,
    }
    assert fake_celery.control.revocations == [("active-task-1", True, "SIGKILL")]
    assert cleared == [(str(run_id), "active-task-1")]
