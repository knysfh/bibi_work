from __future__ import annotations

from typing import Any

from bibi_work_agent.runtime.llm_retry import LlmProviderFailure
from bibi_work_agent.runtime.snapshot_contract import safe_error_message


def run_failure_payload(error: BaseException) -> dict[str, Any]:
    if isinstance(error, LlmProviderFailure):
        detail = safe_error_message(error.original_error)
        info = error.info
        raw_error = {
            "name": error.original_error.__class__.__name__,
            "message": detail,
            "code": info.code,
        }
        if info.status is not None:
            raw_error["status"] = info.status
        payload: dict[str, Any] = {
            "error": info.user_message,
            "message": info.user_message,
            "detail": (
                f"{detail}\nModel request attempts: {error.attempts}; "
                "retry delays: 5s, 15s, 30s."
            ),
            "code": info.code,
            "ownership": info.ownership,
            "retryable": info.retryable,
            "feedback_recommended": False,
            "resolution": {"kind": info.resolution_kind},
            "rawError": raw_error,
        }
        if info.resolution_target is not None:
            payload["resolution"]["target"] = info.resolution_target
        return payload

    detail = safe_error_message(error)
    return {
        "error": (
            "The request could not be completed. Please contact your administrator "
            "if the problem continues."
        ),
        "message": (
            "The request could not be completed. Please contact your administrator "
            "if the problem continues."
        ),
        "detail": detail,
        "code": "BIWORK_INTERNAL_ERROR",
        "ownership": "biwork",
        "retryable": False,
        "feedback_recommended": True,
        "resolution": {"kind": "send_feedback", "target": "feedback"},
        "rawError": {
            "name": error.__class__.__name__,
            "message": detail,
            "code": "BIWORK_INTERNAL_ERROR",
        },
    }
