from __future__ import annotations

from collections.abc import Iterator
from contextlib import contextmanager
from contextvars import ContextVar
from typing import Any


_file_tool_contexts: ContextVar[tuple[dict[str, Any], ...]] = ContextVar(
    "bibi_file_tool_contexts",
    default=(),
)


def remember_file_tool_call(
    *,
    tool_call_id: str | None,
    tool_name: str | None,
    path: str | None,
    operation: str | None,
    args_hash: str | None = None,
    subagent_id: str | None = None,
    subagent_name: str | None = None,
    parent_tool_call_id: str | None = None,
) -> None:
    if not tool_call_id or not tool_name or not path:
        return
    context = {
        "tool_call_id": tool_call_id,
        "tool_name": tool_name,
        "path": path,
        "operation": normalize_operation(operation or tool_name),
    }
    if args_hash:
        context["args_hash"] = args_hash
    for key, value in [
        ("subagent_id", subagent_id),
        ("subagent_name", subagent_name),
        ("parent_tool_call_id", parent_tool_call_id),
    ]:
        if value:
            context[key] = value
    current = [
        item
        for item in _file_tool_contexts.get()
        if item.get("tool_call_id") != tool_call_id
    ]
    _file_tool_contexts.set((*current, context))


@contextmanager
def file_tool_call_context(
    *,
    tool_call_id: str | None,
    tool_name: str | None,
    path: str | None,
    operation: str | None,
    args_hash: str | None = None,
    subagent_id: str | None = None,
    subagent_name: str | None = None,
    parent_tool_call_id: str | None = None,
) -> Iterator[None]:
    if not tool_call_id or not tool_name or not path:
        yield
        return
    token = _file_tool_contexts.set(_file_tool_contexts.get())
    remember_file_tool_call(
        tool_call_id=tool_call_id,
        tool_name=tool_name,
        path=path,
        operation=operation,
        args_hash=args_hash,
        subagent_id=subagent_id,
        subagent_name=subagent_name,
        parent_tool_call_id=parent_tool_call_id,
    )
    try:
        yield
    finally:
        _file_tool_contexts.reset(token)


def current_file_tool_call(path: str, operation: str) -> dict[str, Any] | None:
    expected_operation = normalize_operation(operation)
    for context in reversed(_file_tool_contexts.get()):
        if context.get("path") != path:
            continue
        if normalize_operation(str(context.get("operation") or "")) != expected_operation:
            continue
        return dict(context)
    return None


def clear_file_tool_contexts() -> None:
    _file_tool_contexts.set(())


def normalize_operation(value: str) -> str:
    if value in {"write", "write_file", "file_write"}:
        return "write"
    if value in {"edit", "edit_file", "file_edit"}:
        return "edit"
    if value in {"read", "read_file", "file_read"}:
        return "read"
    return value
