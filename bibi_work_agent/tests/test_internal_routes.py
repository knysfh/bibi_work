from __future__ import annotations

from uuid import uuid4

from fastapi.testclient import TestClient

from bibi_work_agent.api import internal_routes
from bibi_work_agent.api.app import app
from bibi_work_agent.settings import Settings


class FakeTask:
    id = "resume-task"


class FakeResumeTask:
    def __init__(self) -> None:
        self.calls: list[tuple[str, dict]] = []

    def delay(self, run_id: str, payload: dict):
        self.calls.append((run_id, payload))
        return FakeTask()


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
        headers={"Authorization": "Bearer test-token"},
        json={
            "tenant_id": str(uuid4()),
            "conversation_id": str(uuid4()),
            "approval_id": str(uuid4()),
            "decision_payload": {"decision": "approved"},
        },
    )

    assert response.status_code == 200
    assert response.json() == {"status": "queued", "task_id": "resume-task"}
    assert fake_resume.calls[0][0] == str(run_id)
    assert fake_resume.calls[0][1]["decision_payload"] == {"decision": "approved"}
