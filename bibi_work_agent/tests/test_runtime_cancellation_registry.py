from __future__ import annotations

from bibi_work_agent.runtime import cancellation


class FakeRedis:
    def __init__(self) -> None:
        self.values: dict[str, str] = {}

    def set(self, key: str, value: str, *, ex: int) -> bool:  # noqa: ARG002
        self.values[key] = value
        return True

    def get(self, key: str) -> str | None:
        return self.values.get(key)

    def exists(self, key: str) -> bool:
        return key in self.values

    def eval(
        self, script: str, key_count: int, key: str, expected: str
    ) -> int:  # noqa: ARG002
        if self.values.get(key) != expected:
            return 0
        del self.values[key]
        return 1


def test_active_task_registry_only_clears_the_matching_task(monkeypatch) -> None:
    fake_redis = FakeRedis()
    monkeypatch.setattr(cancellation, "_redis_client", lambda: fake_redis)

    assert cancellation.register_active_task("run-1", "task-1")
    assert cancellation.active_task_id("run-1") == "task-1"
    assert not cancellation.clear_active_task("run-1", "stale-task")
    assert cancellation.active_task_id("run-1") == "task-1"
    assert cancellation.clear_active_task("run-1", "task-1")
    assert cancellation.active_task_id("run-1") is None


def test_cancel_marker_remains_independent_from_active_task(monkeypatch) -> None:
    fake_redis = FakeRedis()
    monkeypatch.setattr(cancellation, "_redis_client", lambda: fake_redis)

    assert cancellation.register_active_task("run-1", "task-1")
    assert cancellation.mark_run_cancelled("run-1")
    assert cancellation.is_run_cancelled("run-1")
    assert cancellation.active_task_id("run-1") == "task-1"
