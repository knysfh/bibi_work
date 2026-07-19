from __future__ import annotations

from functools import lru_cache

from bibi_work_agent.settings import settings


CANCEL_KEY_PREFIX = "bibi_work_agent:run_cancel:"
ACTIVE_TASK_KEY_PREFIX = "bibi_work_agent:active_task:"
DEFAULT_CANCEL_TTL_SEC = 24 * 60 * 60
DEFAULT_ACTIVE_TASK_TTL_SEC = 2 * 60 * 60


class RunCancelled(RuntimeError):
    def __init__(self, run_id: str | None, reason: str = "cancelled") -> None:
        super().__init__(reason)
        self.run_id = run_id
        self.reason = reason


def mark_run_cancelled(run_id: str, *, ttl_sec: int = DEFAULT_CANCEL_TTL_SEC) -> bool:
    try:
        client = _redis_client()
        return bool(client.set(_cancel_key(run_id), "1", ex=ttl_sec))
    except Exception:
        return False


def is_run_cancelled(run_id: str) -> bool:
    try:
        return bool(_redis_client().exists(_cancel_key(run_id)))
    except Exception:
        return False


def register_active_task(
    run_id: str,
    task_id: str,
    *,
    ttl_sec: int = DEFAULT_ACTIVE_TASK_TTL_SEC,
) -> bool:
    if not run_id or not task_id:
        return False
    try:
        return bool(_redis_client().set(_active_task_key(run_id), task_id, ex=ttl_sec))
    except Exception:
        return False


def active_task_id(run_id: str) -> str | None:
    try:
        value = _redis_client().get(_active_task_key(run_id))
    except Exception:
        return None
    return str(value) if value else None


def clear_active_task(run_id: str, task_id: str) -> bool:
    if not run_id or not task_id:
        return False
    script = """
    if redis.call('GET', KEYS[1]) == ARGV[1] then
        return redis.call('DEL', KEYS[1])
    end
    return 0
    """
    try:
        return bool(
            _redis_client().eval(script, 1, _active_task_key(run_id), task_id)
        )
    except Exception:
        return False


def _cancel_key(run_id: str) -> str:
    return f"{CANCEL_KEY_PREFIX}{run_id}"


def _active_task_key(run_id: str) -> str:
    return f"{ACTIVE_TASK_KEY_PREFIX}{run_id}"


@lru_cache(maxsize=1)
def _redis_client():
    try:
        import redis
    except Exception as exc:  # noqa: BLE001
        raise RuntimeError(
            "redis package is required for runtime cancellation"
        ) from exc

    return redis.Redis.from_url(settings.celery_broker_url, decode_responses=True)
