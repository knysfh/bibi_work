from __future__ import annotations

import pytest

from bibi_work_agent.runtime.error_reporting import run_failure_payload
from bibi_work_agent.runtime.llm_retry import (
    LlmProviderFailure,
    retry_llm_call,
    retry_llm_stream,
)


class ProviderError(RuntimeError):
    def __init__(self, status_code: int, message: str = "provider error") -> None:
        super().__init__(message)
        self.status_code = status_code


def test_retry_llm_call_uses_three_bounded_delays_then_succeeds() -> None:
    calls = 0
    delays: list[float] = []

    def operation() -> str:
        nonlocal calls
        calls += 1
        if calls < 4:
            raise ProviderError(429, "rate limit")
        return "ok"

    assert retry_llm_call(operation, sleep=delays.append) == "ok"
    assert calls == 4
    assert delays == [5.0, 15.0, 30.0]


def test_retry_llm_call_stops_after_three_retries() -> None:
    calls = 0
    delays: list[float] = []

    def operation() -> str:
        nonlocal calls
        calls += 1
        raise ProviderError(429, "rate limit")

    with pytest.raises(LlmProviderFailure) as exc_info:
        retry_llm_call(operation, sleep=delays.append)

    assert calls == 4
    assert delays == [5.0, 15.0, 30.0]
    assert exc_info.value.attempts == 4
    assert exc_info.value.info.code == "USER_LLM_PROVIDER_RATE_LIMITED"
    assert exc_info.value.info.retryable is True


def test_retry_llm_call_does_not_retry_non_recoverable_provider_error() -> None:
    calls = 0
    delays: list[float] = []

    def operation() -> str:
        nonlocal calls
        calls += 1
        raise ProviderError(401, "invalid API key")

    with pytest.raises(LlmProviderFailure) as exc_info:
        retry_llm_call(operation, sleep=delays.append)

    assert calls == 1
    assert delays == []
    assert exc_info.value.info.code == "USER_LLM_PROVIDER_AUTH_FAILED"
    assert exc_info.value.info.retryable is False


def test_retry_llm_stream_does_not_replay_after_partial_output() -> None:
    calls = 0
    delays: list[float] = []

    def operation():
        nonlocal calls
        calls += 1
        yield "partial"
        raise ProviderError(429, "rate limit")

    stream = retry_llm_stream(operation, sleep=delays.append)
    assert next(stream) == "partial"
    with pytest.raises(LlmProviderFailure):
        next(stream)

    assert calls == 1
    assert delays == []


def test_run_failure_payload_separates_user_message_from_diagnostics() -> None:
    provider_error = ProviderError(429, "rate limit token=secret-value")
    try:
        retry_llm_call(lambda: (_ for _ in ()).throw(provider_error), sleep=lambda _: None)
    except LlmProviderFailure as failure:
        payload = run_failure_payload(failure)
    else:  # pragma: no cover
        raise AssertionError("expected provider failure")

    assert payload["code"] == "USER_LLM_PROVIDER_RATE_LIMITED"
    assert payload["retryable"] is True
    assert "secret-value" not in payload["detail"]
    assert "token=[REDACTED]" in payload["detail"]
    assert "rate limit" not in payload["error"].lower()
    assert payload["rawError"]["status"] == 429
