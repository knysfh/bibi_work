from __future__ import annotations

import ast
import json
import re
from collections.abc import Iterable, Mapping
from typing import Any
from uuid import uuid4

from bibi_work_agent.tools.io_policy import summarize_input, summarize_output
from bibi_work_agent.tools.result_presenter import build_tool_result_views


DEEPAGENTS_BUILTIN_TOOL_NAMES = {
    "write_todos",
    "ls",
    "read_file",
    "write_file",
    "edit_file",
    "glob",
    "grep",
    "execute",
    "task",
    "start_async_task",
    "check_async_task",
    "update_async_task",
    "cancel_async_task",
    "list_async_tasks",
    "compact_conversation",
}

PLATFORM_EVENT_TYPES = {
    "run.started",
    "message.delta",
    "message.completed",
    "tool.call.started",
    "tool.call.delta",
    "tool.call.completed",
    "tool.call.failed",
    "interrupt.requested",
    "approval.requested",
    "task.created",
    "task.updated",
    "task.completed",
    "subagent.started",
    "subagent.completed",
    "file.changed",
    "run.completed",
    "run.failed",
    "run.cancelled",
}


class AgentEventNormalizer:
    def __init__(self, *, run_id: str, trace_id: str | None) -> None:
        self.run_id = run_id
        self.trace_id = trace_id
        self._counter = 0
        self._started_tool_call_ids: set[str] = set()
        self._tool_call_context: dict[str, dict[str, Any]] = {}

    def normalize(self, raw_event: Any) -> list[dict[str, Any]]:
        self._counter += 1
        if isinstance(raw_event, dict):
            return self._dict_to_events(raw_event, include_messages=True)

        if isinstance(raw_event, tuple) and raw_event:
            stream_name = raw_event[0]
            value = raw_event[1] if len(raw_event) > 1 else None
            if stream_name == "messages":
                return self._messages_to_events(value)
            if stream_name == "values" and isinstance(value, dict):
                return self._dict_to_events(value, include_messages=False)

        return []

    def completed_message(self, result: Any) -> dict[str, Any]:
        content = extract_completed_content(result)
        return self._event(
            "message.completed",
            {
                "run_id": self.run_id,
                "content": content,
                "result": json_safe_result(result),
            },
        )

    def _dict_to_events(
        self, raw_event: dict[str, Any], *, include_messages: bool
    ) -> list[dict[str, Any]]:
        explicit_type = raw_event.get("type") or raw_event.get("event")
        if explicit_type in PLATFORM_EVENT_TYPES:
            payload = raw_event.get("payload", raw_event)
            if explicit_type == "message.completed" and isinstance(payload, dict):
                payload = ensure_completed_content(payload)
            return [self._event(explicit_type, payload)]

        if include_messages and "messages" in raw_event:
            return self._messages_to_events(raw_event["messages"])

        if "todos" in raw_event:
            return self._todos_to_events(raw_event["todos"])

        if "__interrupt__" in raw_event:
            return [
                self._event(
                    "interrupt.requested",
                    {
                        "run_id": self.run_id,
                        "interrupt": raw_event["__interrupt__"],
                    },
                )
            ]

        return []

    def _messages_to_events(self, messages: Any) -> list[dict[str, Any]]:
        events: list[dict[str, Any]] = []
        for item in ensure_iterable(messages):
            events.extend(self._tool_call_started_events(item))
            tool_event = self._tool_message_to_event(item)
            if tool_event is not None:
                events.append(tool_event)
                continue
            content = extract_message_content(item)
            if content:
                events.append(
                    self._event(
                        "message.delta", {"run_id": self.run_id, "content": content}
                    )
                )
        return events

    def _tool_call_started_events(self, message: Any) -> list[dict[str, Any]]:
        events: list[dict[str, Any]] = []
        for tool_call in extract_tool_calls(message):
            tool_name = tool_call["name"]
            if not is_deepagents_builtin_tool(tool_name):
                continue
            tool_call_id = tool_call["id"]
            input_summary = summarize_input(tool_call["args"])
            self._tool_call_context[tool_call_id] = {
                "tool_name": tool_name,
                "input_summary": input_summary,
                "args": tool_call["args"],
            }
            if tool_call_id in self._started_tool_call_ids:
                continue
            self._started_tool_call_ids.add(tool_call_id)
            events.append(
                self._event(
                    "tool.call.started",
                    {
                        "run_id": self.run_id,
                        "tool_call_id": tool_call_id,
                        "tool_name": tool_name,
                        "status": "running",
                        "input_summary": input_summary,
                    },
                )
            )
        return events

    def _tool_message_to_event(self, message: Any) -> dict[str, Any] | None:
        if not is_tool_message(message):
            return None

        tool_call_id = str(getattr(message, "tool_call_id", "") or uuid4())
        context = self._tool_call_context.get(tool_call_id, {})
        tool_name = str(
            getattr(message, "name", "")
            or context.get("tool_name")
            or ""
        )
        if not is_deepagents_builtin_tool(tool_name):
            return None

        status = str(getattr(message, "status", "") or "success")
        content = extract_tool_message_content(message)
        completed = status != "error"
        payload: dict[str, Any] = {
            "run_id": self.run_id,
            "tool_call_id": tool_call_id,
            "tool_name": tool_name,
            "status": "completed" if completed else "failed",
        }
        input_summary = context.get("input_summary")
        if input_summary:
            payload["input_summary"] = input_summary

        if completed:
            result_value = structured_tool_result_value(tool_name, content)
            payload["output_summary"] = summarize_output(result_value, None)
            views = build_tool_result_views(result_value)
            if views:
                payload["views"] = views
            return self._event("tool.call.completed", payload)

        payload["error_summary"] = summarize_output(content, None)
        return self._event("tool.call.failed", payload)

    def _todos_to_events(self, todos: Any) -> list[dict[str, Any]]:
        events: list[dict[str, Any]] = []
        for item in ensure_iterable(todos):
            events.append(
                self._event("task.updated", {"run_id": self.run_id, "task": item})
            )
        return events

    def _event(self, event_type: str, payload: dict[str, Any]) -> dict[str, Any]:
        return {
            "event_id": f"{event_type}.{self.run_id}.{self._counter}.{uuid4()}",
            "type": event_type,
            "payload": payload,
            "trace_id": self.trace_id,
        }


def ensure_iterable(value: Any) -> Iterable[Any]:
    if value is None:
        return []
    if isinstance(value, (list, tuple)):
        return value
    return [value]


def extract_message_content(message: Any) -> str | None:
    if is_tool_message(message):
        return None
    if isinstance(message, str):
        return message
    if isinstance(message, dict):
        content = message.get("content")
        if isinstance(content, str):
            return content
        if content is not None:
            return safe_repr(content)
    content = getattr(message, "content", None)
    if isinstance(content, str):
        return content
    if content is not None:
        return safe_repr(content)
    return None


def ensure_completed_content(payload: dict[str, Any]) -> dict[str, Any]:
    if payload.get("content"):
        return payload
    if "result" not in payload:
        return payload
    return {**payload, "content": extract_completed_content(payload["result"])}


def extract_completed_content(result: Any) -> str:
    if isinstance(result, str):
        return result
    if isinstance(result, dict):
        for key in ("content", "message"):
            value = result.get(key)
            if isinstance(value, str):
                return value
        messages = result.get("messages")
        content = latest_message_content(messages)
        if content:
            return content
    content = getattr(result, "content", None)
    if isinstance(content, str):
        return content
    messages = getattr(result, "messages", None)
    content = latest_message_content(messages)
    if content:
        return content
    return safe_repr(result)


def is_tool_message(message: Any) -> bool:
    if getattr(message, "type", None) == "tool":
        return True
    if isinstance(message, dict) and message.get("type") == "tool":
        return True
    return False


def is_deepagents_builtin_tool(tool_name: str) -> bool:
    return tool_name in DEEPAGENTS_BUILTIN_TOOL_NAMES


def extract_tool_calls(message: Any) -> list[dict[str, Any]]:
    raw_calls = raw_tool_calls(message)
    calls: list[dict[str, Any]] = []
    for raw_call in ensure_iterable(raw_calls):
        call = normalize_tool_call(raw_call)
        if call is not None:
            calls.append(call)
    return calls


def raw_tool_calls(message: Any) -> Any:
    if isinstance(message, dict):
        return message.get("tool_calls") or message.get("toolCalls")
    return getattr(message, "tool_calls", None)


def normalize_tool_call(raw_call: Any) -> dict[str, Any] | None:
    if raw_call is None:
        return None
    call_id = raw_value(raw_call, "id") or raw_value(raw_call, "tool_call_id")
    name = raw_value(raw_call, "name")
    args = raw_value(raw_call, "args")

    function = raw_value(raw_call, "function")
    if isinstance(function, Mapping):
        name = name or function.get("name")
        args = args if args is not None else function.get("arguments")

    if not call_id or not name:
        return None
    return {
        "id": str(call_id),
        "name": str(name),
        "args": normalize_tool_args(args),
    }


def raw_value(value: Any, key: str) -> Any:
    if isinstance(value, Mapping):
        return value.get(key)
    return getattr(value, key, None)


def normalize_tool_args(args: Any) -> Any:
    if isinstance(args, str):
        parsed = parse_literal_content(args)
        return parsed if parsed is not None else args
    if args is None:
        return {}
    return args


def extract_tool_message_content(message: Any) -> str:
    content = (
        message.get("content")
        if isinstance(message, dict)
        else getattr(message, "content", "")
    )
    if isinstance(content, str):
        return content
    if content is None:
        return ""
    return safe_repr(content)


def structured_tool_result_value(tool_name: str, content: str) -> Any:
    parsed = parse_literal_content(content)
    if tool_name in {"ls", "glob"} and isinstance(parsed, list):
        return path_rows(parsed)
    if tool_name == "grep":
        return grep_result_value(content)
    if tool_name == "write_todos":
        return write_todos_result_value(content)
    if tool_name == "write_file":
        return write_file_result_value(content)
    if tool_name == "edit_file":
        return edit_file_result_value(content)
    if tool_name == "execute":
        return execute_result_value(content)
    return parsed if parsed is not None else content


def parse_literal_content(content: str) -> Any:
    stripped = content.strip()
    if not stripped or stripped[0] not in "[{":
        return None
    try:
        return json.loads(stripped)
    except json.JSONDecodeError:
        pass
    try:
        return ast.literal_eval(stripped)
    except (SyntaxError, ValueError):
        return None


def path_rows(value: list[Any]) -> list[dict[str, str]]:
    rows: list[dict[str, str]] = []
    for item in value:
        path = str(item)
        rows.append(
            {
                "path": path,
                "type": "directory" if path.endswith("/") else "file",
            }
        )
    return rows


def grep_result_value(content: str) -> Any:
    stripped = content.strip()
    if not stripped or stripped == "No matches found":
        return content
    rows = grep_content_rows(stripped)
    if rows:
        return rows
    rows = grep_count_rows(stripped)
    if rows:
        return rows
    paths = [line.strip() for line in stripped.splitlines() if line.strip()]
    if paths and all(path.startswith("/") for path in paths):
        return path_rows(paths)
    return content


def grep_content_rows(content: str) -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    current_path: str | None = None
    for line in content.splitlines():
        if line.startswith("/") and line.endswith(":"):
            current_path = line[:-1]
            continue
        match = re.match(r"^\s+(\d+):\s?(.*)$", line)
        if current_path and match:
            rows.append(
                {
                    "path": current_path,
                    "line": int(match.group(1)),
                    "text": match.group(2),
                }
            )
            continue
        return []
    return rows


def grep_count_rows(content: str) -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    for line in content.splitlines():
        match = re.match(r"^(.+):\s+(\d+)$", line.strip())
        if not match:
            return []
        rows.append({"path": match.group(1), "count": int(match.group(2))})
    return rows


def write_todos_result_value(content: str) -> Any:
    prefix = "Updated todo list to "
    if not content.startswith(prefix):
        return content
    parsed = parse_literal_content(content[len(prefix) :])
    return parsed if parsed is not None else content


def write_file_result_value(content: str) -> Any:
    prefix = "Updated file "
    if not content.startswith(prefix):
        return content
    return {"operation": "write_file", "path": content[len(prefix) :].strip()}


def edit_file_result_value(content: str) -> Any:
    match = re.match(
        r"^Successfully replaced (\d+) instance\(s\) of the string in '(.+)'$",
        content.strip(),
    )
    if not match:
        return content
    return {
        "operation": "edit_file",
        "occurrences": int(match.group(1)),
        "path": match.group(2),
    }


def execute_result_value(content: str) -> Any:
    exit_match = re.search(
        r"\n\[Command (succeeded|failed) with exit code (-?\d+)\]",
        content,
    )
    truncated = "[Output was truncated due to size limits]" in content
    if not exit_match and not truncated:
        return content

    output = content[: exit_match.start()] if exit_match else content
    output = output.replace("\n[Output was truncated due to size limits]", "")
    result: dict[str, Any] = {"output": output}
    if exit_match:
        result["exit_code"] = int(exit_match.group(2))
        result["command_status"] = exit_match.group(1)
    if truncated:
        result["truncated"] = True
    return result


def latest_message_content(messages: Any) -> str | None:
    latest: str | None = None
    for item in ensure_iterable(messages):
        content = extract_message_content(item)
        if content:
            latest = content
    return latest


def json_safe_result(value: Any) -> Any:
    try:
        json.dumps(value)
    except (TypeError, ValueError):
        return safe_repr(value)
    return value


def safe_repr(value: Any) -> str:
    text = str(value)
    return text if len(text) <= 4000 else text[:4000]
