from __future__ import annotations

import re
from typing import Any


SENSITIVE_SNAPSHOT_KEYS = {
    "apikey",
    "api_key",
    "accesstoken",
    "access_token",
    "authorization",
    "password",
    "refreshtoken",
    "refresh_token",
    "secret",
    "secretref",
    "secret_ref",
    "token",
}
RUNTIME_EXTENSION_CONTRIBUTION_TYPES = {
    "assistant",
    "agent",
    "skill",
    "mcp_server",
    "channel_plugin",
    "acp_adapter",
}

SENSITIVE_ERROR_PATTERN = re.compile(
    r"(?i)(bearer\s+)[a-z0-9._\-+/=]+|"
    r"(api[_-]?key|access[_-]?token|authorization|password|refresh[_-]?token|secret[_-]?ref|secret|token)"
    r"([=:]\s*)"
    r"(?:bearer\s+)?"
    r"([^,\s}\]]+)"
)


def validate_run_config_snapshot(snapshot: Any) -> None:
    if not isinstance(snapshot, dict):
        raise RuntimeError("run_config_snapshot must be an object")

    forbidden_path = first_forbidden_secret_path(snapshot)
    if forbidden_path:
        raise RuntimeError(
            f"run_config_snapshot contains forbidden secret material at {forbidden_path}"
        )

    runtime = snapshot.get("runtime")
    if not isinstance(runtime, dict):
        raise RuntimeError("run_config_snapshot.runtime.kind is required")
    runtime_kind = str(runtime.get("kind") or "").strip()
    if runtime_kind != "deepagents":
        raise RuntimeError(
            f"runtime.kind={runtime_kind or '<missing>'} is not handled by Python runtime"
        )

    require_snapshot_field(snapshot, "tenant_id")
    require_snapshot_field(snapshot, "run_id")

    actor = snapshot.get("actor")
    if not isinstance(actor, dict):
        raise RuntimeError("run_config_snapshot.actor.user_id is required")
    require_snapshot_field(actor, "user_id", prefix="run_config_snapshot.actor")

    agent = snapshot.get("agent")
    if not isinstance(agent, dict):
        raise RuntimeError("run_config_snapshot.agent must be an object")

    model = snapshot.get("model")
    if model is None:
        model = agent.get("model")
    if not isinstance(model, dict):
        raise RuntimeError("run_config_snapshot.model is required")

    require_snapshot_list(snapshot, "tools")
    require_snapshot_list(snapshot, "skills")
    require_snapshot_list(snapshot, "mcp_tools")
    require_optional_snapshot_list(snapshot, "sql_tools")

    workspace = snapshot.get("workspace")
    if not isinstance(workspace, dict):
        raise RuntimeError("run_config_snapshot.workspace must be an object")
    local_mounts = workspace.get("local_mounts")
    if not isinstance(local_mounts, list):
        raise RuntimeError("run_config_snapshot.workspace.local_mounts must be a list")

    ui = snapshot.get("ui")
    if not isinstance(ui, dict):
        raise RuntimeError("run_config_snapshot.ui.client is required")
    require_snapshot_field(ui, "client", prefix="run_config_snapshot.ui")

    validate_extension_contributions(snapshot.get("extension_contributions"))


def reject_non_python_runtime_kind(snapshot: dict[str, Any]) -> None:
    """Fail before Python emits run.started for desktop-only runtime snapshots."""
    if not isinstance(snapshot, dict):
        return
    runtime = snapshot.get("runtime")
    if not isinstance(runtime, dict):
        return
    runtime_kind = str(runtime.get("kind") or "").strip()
    if runtime_kind and runtime_kind != "deepagents":
        raise RuntimeError(
            f"runtime.kind={runtime_kind} is not handled by Python runtime"
        )


def validate_extension_contributions(value: Any) -> None:
    if value is None:
        return
    if not isinstance(value, list):
        raise RuntimeError("run_config_snapshot.extension_contributions must be a list")

    for index, contribution in enumerate(value):
        prefix = f"run_config_snapshot.extension_contributions[{index}]"
        if not isinstance(contribution, dict):
            raise RuntimeError(f"{prefix} must be an object")
        contribution_type = str(contribution.get("type") or "").strip()
        if contribution_type not in RUNTIME_EXTENSION_CONTRIBUTION_TYPES:
            raise RuntimeError(f"{prefix}.type is not allowed for Python runtime")
        require_snapshot_field(contribution, "key", prefix=prefix)
        manifest = contribution.get("manifest")
        if not isinstance(manifest, dict):
            raise RuntimeError(f"{prefix}.manifest must be an object")


def require_snapshot_field(
    value: dict[str, Any], key: str, *, prefix: str = "run_config_snapshot"
) -> None:
    item = value.get(key)
    if item is None or (isinstance(item, str) and not item.strip()):
        raise RuntimeError(f"{prefix}.{key} is required")


def require_snapshot_list(
    value: dict[str, Any], key: str, *, prefix: str = "run_config_snapshot"
) -> None:
    item = value.get(key)
    if not isinstance(item, list):
        raise RuntimeError(f"{prefix}.{key} must be a list")


def require_optional_snapshot_list(
    value: dict[str, Any], key: str, *, prefix: str = "run_config_snapshot"
) -> None:
    item = value.get(key)
    if item is not None and not isinstance(item, list):
        raise RuntimeError(f"{prefix}.{key} must be a list")


def first_forbidden_secret_path(
    value: Any, path: str = "run_config_snapshot"
) -> str | None:
    if isinstance(value, dict):
        is_json_schema_properties = path.endswith(".properties") and ".schema." in path
        for key, item in value.items():
            key_text = str(key)
            child_path = f"{path}.{key_text}"
            normalized = re.sub(r"[^a-z0-9]+", "_", key_text.lower()).strip("_")
            if normalized in SENSITIVE_SNAPSHOT_KEYS and not is_json_schema_properties:
                return child_path
            found = first_forbidden_secret_path(item, child_path)
            if found:
                return found
    elif isinstance(value, list):
        for index, item in enumerate(value):
            found = first_forbidden_secret_path(item, f"{path}[{index}]")
            if found:
                return found
    return None


def safe_error_message(error: BaseException) -> str:
    text = str(error)
    if not text:
        return error.__class__.__name__

    def replace(match: re.Match[str]) -> str:
        if match.group(1):
            return f"{match.group(1)}[REDACTED]"
        return f"{match.group(2)}{match.group(3)}[REDACTED]"

    return SENSITIVE_ERROR_PATTERN.sub(replace, text)
