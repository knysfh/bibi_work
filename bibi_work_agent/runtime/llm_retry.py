from __future__ import annotations

import asyncio
import logging
import time
from collections.abc import AsyncIterable, AsyncIterator, Callable, Iterable, Iterator
from dataclasses import dataclass
from typing import Any, TypeVar

import httpx
import openai


LLM_RETRY_DELAYS_SECONDS = (5.0, 15.0, 30.0)

T = TypeVar("T")
log = logging.getLogger(__name__)


@dataclass(frozen=True)
class LlmErrorInfo:
    code: str
    user_message: str
    ownership: str
    retryable: bool
    resolution_kind: str
    resolution_target: str | None = None
    status: int | None = None


class LlmProviderFailure(RuntimeError):
    def __init__(
        self,
        info: LlmErrorInfo,
        original_error: BaseException,
        *,
        attempts: int,
    ) -> None:
        super().__init__(info.user_message)
        self.info = info
        self.original_error = original_error
        self.attempts = attempts


def classify_llm_error(error: BaseException) -> LlmErrorInfo | None:
    status = error_status(error)
    message = str(error).lower()

    if status == 429 or "too many requests" in message or "rate limit" in message:
        return LlmErrorInfo(
            code="USER_LLM_PROVIDER_RATE_LIMITED",
            user_message="The model service is temporarily busy. Please try again later.",
            ownership="user_llm_provider",
            retryable=True,
            resolution_kind="retry",
            status=status or 429,
        )
    if status == 408 or isinstance(
        error, (openai.APITimeoutError, httpx.TimeoutException)
    ):
        return LlmErrorInfo(
            code="USER_LLM_PROVIDER_TIMEOUT",
            user_message="The model service did not respond in time. Please try again later.",
            ownership="user_llm_provider",
            retryable=True,
            resolution_kind="retry",
            status=status or 408,
        )
    if isinstance(error, (openai.APIConnectionError, httpx.TransportError)):
        return LlmErrorInfo(
            code="USER_LLM_PROVIDER_NETWORK_ERROR",
            user_message="The model service cannot be reached right now. Please try again later.",
            ownership="user_llm_provider",
            retryable=True,
            resolution_kind="retry",
            status=status,
        )
    if status == 409 or (status is not None and status >= 500):
        return LlmErrorInfo(
            code="USER_LLM_PROVIDER_GATEWAY_ERROR",
            user_message="The model service is temporarily unavailable. Please try again later.",
            ownership="user_llm_provider",
            retryable=True,
            resolution_kind="retry",
            status=status,
        )
    if status == 401:
        return provider_configuration_error(
            code="USER_LLM_PROVIDER_AUTH_FAILED",
            resolution_kind="check_provider_credentials",
            status=status,
        )
    if status == 402:
        return provider_configuration_error(
            code="USER_LLM_PROVIDER_BILLING_REQUIRED",
            resolution_kind="check_provider_billing",
            status=status,
        )
    if status == 403:
        return provider_configuration_error(
            code="USER_LLM_PROVIDER_PERMISSION_DENIED",
            resolution_kind="check_provider_credentials",
            status=status,
        )
    if status == 404:
        code = (
            "USER_LLM_PROVIDER_MODEL_NOT_FOUND"
            if "model" in message
            else "USER_LLM_PROVIDER_ENDPOINT_NOT_FOUND"
        )
        return provider_configuration_error(
            code=code,
            resolution_kind=(
                "change_model"
                if code == "USER_LLM_PROVIDER_MODEL_NOT_FOUND"
                else "check_provider_base_url"
            ),
            status=status,
        )
    if status in {400, 413, 422}:
        if any(
            marker in message
            for marker in ("context length", "context window", "too many tokens")
        ):
            return provider_configuration_error(
                code="USER_LLM_PROVIDER_CONTEXT_TOO_LARGE",
                resolution_kind="reduce_context",
                status=status,
            )
        if "tool" in message and any(
            marker in message for marker in ("schema", "function", "invalid")
        ):
            return provider_configuration_error(
                code="USER_LLM_PROVIDER_INVALID_TOOL_SCHEMA",
                resolution_kind="send_feedback",
                status=status,
            )
        return provider_configuration_error(
            code="USER_LLM_PROVIDER_INVALID_REQUEST",
            resolution_kind="send_feedback",
            status=status,
        )
    return None


def provider_configuration_error(
    *, code: str, resolution_kind: str, status: int | None
) -> LlmErrorInfo:
    return LlmErrorInfo(
        code=code,
        user_message=(
            "The model service cannot complete this request with the current "
            "configuration. Please contact your administrator."
        ),
        ownership="user_llm_provider",
        retryable=False,
        resolution_kind=resolution_kind,
        resolution_target="provider_settings",
        status=status,
    )


def error_status(error: BaseException) -> int | None:
    status = getattr(error, "status_code", None)
    if isinstance(status, int):
        return status
    status = getattr(error, "status", None)
    if isinstance(status, int):
        return status
    response = getattr(error, "response", None)
    status = getattr(response, "status_code", None)
    return status if isinstance(status, int) else None


def record_retry(info: LlmErrorInfo, *, attempts: int, delay: float) -> None:
    log.warning(
        "Retrying LLM request after recoverable provider error: "
        "code=%s status=%s retry=%s/%s delay_seconds=%s",
        info.code,
        info.status,
        attempts,
        len(LLM_RETRY_DELAYS_SECONDS),
        delay,
    )


def retry_llm_call(
    operation: Callable[[], T],
    *,
    sleep: Callable[[float], Any] | None = None,
) -> T:
    sleeper = sleep or time.sleep
    attempts = 0
    while True:
        attempts += 1
        try:
            return operation()
        except Exception as error:  # noqa: BLE001 - provider SDKs vary by gateway.
            info = classify_llm_error(error)
            if info is None:
                raise
            retry_index = attempts - 1
            if not info.retryable or retry_index >= len(LLM_RETRY_DELAYS_SECONDS):
                raise LlmProviderFailure(
                    info, error, attempts=attempts
                ) from error
            delay = LLM_RETRY_DELAYS_SECONDS[retry_index]
            record_retry(info, attempts=attempts, delay=delay)
            sleeper(delay)


async def retry_llm_call_async(
    operation: Callable[[], Any],
    *,
    sleep: Callable[[float], Any] | None = None,
) -> T:
    sleeper = sleep or asyncio.sleep
    attempts = 0
    while True:
        attempts += 1
        try:
            return await operation()
        except Exception as error:  # noqa: BLE001 - provider SDKs vary by gateway.
            info = classify_llm_error(error)
            if info is None:
                raise
            retry_index = attempts - 1
            if not info.retryable or retry_index >= len(LLM_RETRY_DELAYS_SECONDS):
                raise LlmProviderFailure(
                    info, error, attempts=attempts
                ) from error
            delay = LLM_RETRY_DELAYS_SECONDS[retry_index]
            record_retry(info, attempts=attempts, delay=delay)
            await sleeper(delay)


def retry_llm_stream(
    operation: Callable[[], Iterable[T]],
    *,
    sleep: Callable[[float], Any] | None = None,
) -> Iterator[T]:
    sleeper = sleep or time.sleep
    attempts = 0
    while True:
        attempts += 1
        emitted = False
        try:
            for item in operation():
                emitted = True
                yield item
            return
        except Exception as error:  # noqa: BLE001 - provider SDKs vary by gateway.
            info = classify_llm_error(error)
            if info is None:
                raise
            retry_index = attempts - 1
            if (
                emitted
                or not info.retryable
                or retry_index >= len(LLM_RETRY_DELAYS_SECONDS)
            ):
                raise LlmProviderFailure(
                    info, error, attempts=attempts
                ) from error
            delay = LLM_RETRY_DELAYS_SECONDS[retry_index]
            record_retry(info, attempts=attempts, delay=delay)
            sleeper(delay)


async def retry_llm_stream_async(
    operation: Callable[[], AsyncIterable[T]],
    *,
    sleep: Callable[[float], Any] | None = None,
) -> AsyncIterator[T]:
    sleeper = sleep or asyncio.sleep
    attempts = 0
    while True:
        attempts += 1
        emitted = False
        try:
            async for item in operation():
                emitted = True
                yield item
            return
        except Exception as error:  # noqa: BLE001 - provider SDKs vary by gateway.
            info = classify_llm_error(error)
            if info is None:
                raise
            retry_index = attempts - 1
            if (
                emitted
                or not info.retryable
                or retry_index >= len(LLM_RETRY_DELAYS_SECONDS)
            ):
                raise LlmProviderFailure(
                    info, error, attempts=attempts
                ) from error
            delay = LLM_RETRY_DELAYS_SECONDS[retry_index]
            record_retry(info, attempts=attempts, delay=delay)
            await sleeper(delay)
