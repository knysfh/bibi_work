from __future__ import annotations

from dataclasses import dataclass
from functools import lru_cache

from bibi_work_agent.settings import settings


RESUME_KEY_PREFIX = "bibi_work_agent:approval_resume:"
DEFAULT_RESUME_TTL_SEC = 24 * 60 * 60


@dataclass(frozen=True)
class ResumeLease:
    approval_id: str
    acquired: bool


class ResumeIdempotencyStore:
    """Redis-backed guard for approval resume at-most-once execution."""

    def __init__(self, *, ttl_sec: int = DEFAULT_RESUME_TTL_SEC) -> None:
        self.ttl_sec = ttl_sec

    def acquire(self, approval_id: str) -> ResumeLease:
        key = _resume_key(approval_id)
        try:
            acquired = bool(
                _redis_client().eval(
                    """
                    local current = redis.call('GET', KEYS[1])
                    if current == false or current == 'failed' then
                        redis.call('SET', KEYS[1], 'running', 'EX', ARGV[1])
                        return 1
                    end
                    return 0
                    """,
                    1,
                    key,
                    self.ttl_sec,
                )
            )
        except Exception as exc:  # noqa: BLE001
            raise RuntimeError("resume idempotency store is unavailable") from exc
        return ResumeLease(approval_id=approval_id, acquired=acquired)

    def mark_completed(self, approval_id: str) -> None:
        self._mark_terminal(approval_id, "completed")

    def mark_failed(self, approval_id: str) -> None:
        self._mark_terminal(approval_id, "failed")

    def _mark_terminal(self, approval_id: str, status: str) -> None:
        try:
            _redis_client().set(_resume_key(approval_id), status, ex=self.ttl_sec)
        except Exception as exc:  # noqa: BLE001
            raise RuntimeError("resume idempotency store is unavailable") from exc


def _resume_key(approval_id: str) -> str:
    return f"{RESUME_KEY_PREFIX}{approval_id}"


@lru_cache(maxsize=1)
def _redis_client():
    try:
        import redis
    except Exception as exc:  # noqa: BLE001
        raise RuntimeError("redis package is required for resume idempotency") from exc

    return redis.Redis.from_url(settings.celery_broker_url, decode_responses=True)
