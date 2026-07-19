from __future__ import annotations

from bibi_work_agent.tools.io_policy import (
    apply_output_policy,
    redact_inline_secret,
    summarize_input,
    summarize_output,
)


def test_summarize_input_redacts_nested_secrets() -> None:
    summary = summarize_input(
        {
            "kwargs": {
                "path": "/workspace/a.txt",
                "password": "plain-password",
                "nested": {"api_token": "secret-token"},
            }
        }
    )

    assert "plain-password" not in summary
    assert "secret-token" not in summary
    assert "[REDACTED]" in summary
    assert "/workspace/a.txt" in summary


def test_redact_inline_secret_redacts_authorization_bearer_values() -> None:
    redacted = redact_inline_secret(
        "provider failed authorization: Bearer raw-secret Bearer standalone-secret",
        redact_fields=("authorization", "token", "secret", "bearer"),
    )

    assert "raw-secret" not in redacted
    assert "standalone-secret" not in redacted
    assert "authorization:[REDACTED]" in redacted
    assert "Bearer [REDACTED]" in redacted


def test_apply_output_policy_redacts_structured_values() -> None:
    output = apply_output_policy(
        {
            "status": "ok",
            "token": "secret-token",
            "nested": {"password": "plain-password"},
        },
        {
            "redact_fields": ["token", "password"],
            "max_output_bytes": 10_000,
        },
    )

    assert output == {
        "status": "ok",
        "token": "[REDACTED]",
        "nested": {"password": "[REDACTED]"},
    }


def test_apply_output_policy_truncates_large_text() -> None:
    output = apply_output_policy(
        "x" * 100,
        {"redact_fields": [], "max_output_bytes": 32},
    )

    assert output.endswith("...[truncated]")
    assert len(output.encode("utf-8")) <= 32


def test_summarize_output_applies_obligations() -> None:
    summary = summarize_output(
        {"token": "secret-token", "content": "safe"},
        {"redact_fields": ["token"], "max_output_bytes": 10_000},
    )

    assert "secret-token" not in summary
    assert "[REDACTED]" in summary


def test_summarize_output_truncates_summary() -> None:
    summary = summarize_output(
        {"content": "x" * 2000},
        {"redact_fields": [], "max_output_bytes": 10_000},
    )

    assert len(summary.encode("utf-8")) <= 1_000
