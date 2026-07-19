from __future__ import annotations

from uuid import uuid4

import pytest

from bibi_work_agent.runtime.snapshot_contract import (
    safe_error_message,
    validate_run_config_snapshot,
)


def valid_run_snapshot() -> dict:
    return {
        "runtime": {"kind": "deepagents"},
        "tenant_id": str(uuid4()),
        "run_id": str(uuid4()),
        "actor": {"user_id": str(uuid4())},
        "agent": {
            "id": str(uuid4()),
            "name": "test-agent",
            "model": {
                "provider": "openai-compatible",
                "model_name": "test-model",
                "credential": {
                    "credential_id": str(uuid4()),
                    "has_secret_ref": True,
                    "runtime_credential_id": "runtime-credential-1",
                },
            },
        },
        "tools": [],
        "skills": [],
        "mcp_tools": [],
        "sql_tools": [],
        "extension_contributions": [
            {
                "extension_package_id": str(uuid4()),
                "extension_name": "acme-tools",
                "type": "mcp_server",
                "key": "acme-mcp",
                "risk_level": "moderate",
                "manifest": {"label": "Acme MCP"},
            }
        ],
        "workspace": {
            "workspace_id": str(uuid4()),
            "remote_project_id": None,
            "local_mounts": [],
        },
        "ui": {"client": "biwork", "conversation_type": "acp"},
    }


def test_validate_run_config_snapshot_accepts_p0_contract() -> None:
    validate_run_config_snapshot(valid_run_snapshot())


@pytest.mark.parametrize(
    ("patch", "message"),
    [
        ({"runtime": {}}, "runtime.kind"),
        ({"tenant_id": ""}, "tenant_id"),
        ({"run_id": None}, "run_id"),
        ({"actor": {}}, "actor.user_id"),
        ({"agent": None}, "agent"),
        ({"agent": {"model": None}}, "model"),
        ({"tools": {}}, "tools"),
        ({"skills": {}}, "skills"),
        ({"mcp_tools": {}}, "mcp_tools"),
        ({"sql_tools": {}}, "sql_tools"),
        ({"workspace": None}, "workspace"),
        ({"workspace": {"local_mounts": {}}}, "workspace.local_mounts"),
        ({"ui": None}, "ui.client"),
        ({"ui": {"client": ""}}, "ui.client"),
    ],
)
def test_validate_run_config_snapshot_requires_p0_fields(
    patch: dict, message: str
) -> None:
    snapshot = valid_run_snapshot()
    snapshot.update(patch)

    with pytest.raises(RuntimeError, match=message):
        validate_run_config_snapshot(snapshot)


@pytest.mark.parametrize("runtime_kind", ["biwork_cli", "disabled"])
def test_validate_run_config_snapshot_rejects_non_python_runtime(runtime_kind: str) -> None:
    snapshot = valid_run_snapshot()
    snapshot["runtime"] = {"kind": runtime_kind}

    with pytest.raises(RuntimeError, match="not handled by Python runtime"):
        validate_run_config_snapshot(snapshot)


def test_validate_run_config_snapshot_rejects_secret_material_without_leaking_value() -> None:
    snapshot = valid_run_snapshot()
    snapshot["agent"]["model"]["credential"]["secret_ref"] = "env://OPENAI_API_KEY"

    with pytest.raises(RuntimeError) as exc_info:
        validate_run_config_snapshot(snapshot)

    message = str(exc_info.value)
    assert "secret_ref" in message
    assert "OPENAI_API_KEY" not in message


def test_validate_run_config_snapshot_allows_sensitive_parameter_names_in_json_schema() -> None:
    snapshot = valid_run_snapshot()
    snapshot["mcp_tools"] = [
        {
            "schema": {
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "password": {"type": "string"},
                        "api_key": {"type": "string"},
                    },
                }
            }
        }
    ]

    validate_run_config_snapshot(snapshot)


def test_validate_run_config_snapshot_still_rejects_secrets_inside_schema_property_definition() -> None:
    snapshot = valid_run_snapshot()
    snapshot["mcp_tools"] = [
        {
            "schema": {
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "password": {"type": "string", "secret": "plain-secret"},
                    },
                }
            }
        }
    ]

    with pytest.raises(RuntimeError) as exc_info:
        validate_run_config_snapshot(snapshot)

    message = str(exc_info.value)
    assert "properties.password.secret" in message
    assert "plain-secret" not in message


def test_validate_run_config_snapshot_rejects_ui_only_extension_contribution() -> None:
    snapshot = valid_run_snapshot()
    snapshot["extension_contributions"][0]["type"] = "theme"

    with pytest.raises(RuntimeError, match="not allowed for Python runtime"):
        validate_run_config_snapshot(snapshot)


def test_validate_run_config_snapshot_rejects_extension_contribution_secret_material() -> None:
    snapshot = valid_run_snapshot()
    snapshot["extension_contributions"][0]["manifest"]["token"] = "plain-token"

    with pytest.raises(RuntimeError) as exc_info:
        validate_run_config_snapshot(snapshot)

    message = str(exc_info.value)
    assert "manifest.token" in message
    assert "plain-token" not in message


def test_safe_error_message_redacts_secret_material() -> None:
    message = safe_error_message(
        RuntimeError(
            "provider failed api_key=sk-test token=plain authorization: Bearer raw-secret Bearer standalone-secret"
        )
    )

    assert "sk-test" not in message
    assert "plain" not in message
    assert "raw-secret" not in message
    assert "standalone-secret" not in message
    assert "api_key=[REDACTED]" in message
    assert "authorization: [REDACTED]" in message
    assert "Bearer [REDACTED]" in message
