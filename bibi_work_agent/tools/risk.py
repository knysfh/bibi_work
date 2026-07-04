from __future__ import annotations

from typing import Any


def classify_tool_risk(tool_name: str, args: dict[str, Any] | None = None) -> str:
    lowered = tool_name.lower()
    if lowered.startswith(("read", "list", "glob", "search", "grep")):
        return "low"
    if lowered.startswith(("write", "edit", "patch", "upload")):
        return "medium"
    if "sql" in lowered and any(word in lowered for word in ("write", "ddl", "exec")):
        return "critical"
    if "local" in lowered or "command" in lowered or "shell" in lowered:
        return "critical"
    if lowered.startswith(("delete", "send", "publish", "deploy")):
        return "high"
    _ = args
    return "medium"
