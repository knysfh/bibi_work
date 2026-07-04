from __future__ import annotations

from functools import lru_cache

from bibi_work_agent.settings import settings


CANCEL_KEY_PREFIX = "bibi_work_agent:run_cancel:"
DEFAULT_CANCEL_TTL_SEC = 24 * 60 * 60


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


def _cancel_key(run_id: str) -> str:
    return f"{CANCEL_KEY_PREFIX}{run_id}"


@lru_cache(maxsize=1)
def _redis_client():
    try:
        import redis
    except Exception as exc:  # noqa: BLE001
        raise RuntimeError(
            "redis package is required for runtime cancellation"
        ) from exc

    return redis.Redis.from_url(settings.celery_broker_url, decode_responses=True)
