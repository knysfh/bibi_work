from __future__ import annotations

import ast
import json
import re
from collections.abc import Iterable, Mapping
from typing import Any
from uuid import uuid4

from bibi_work_agent.tools.io_policy import summarize_input, summarize_output
from bibi_work_agent.tools.result_presenter import build_tool_result_views
from bibi_work_agent.runtime.tool_context import (
    clear_file_tool_contexts,
    remember_file_tool_call,
)


DEEPAGENTS_BUILTIN_TOOL_NAMES = {
    "write_todos",
    "ls",
    "read_file",
    "file_read",
    "write_file",
    "file_write",
    "edit_file",
    "file_edit",
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

FILE_TOOL_OPERATIONS = {
    "read_file": "read",
    "file_read": "read",
    "write_file": "write",
    "file_write": "write",
    "edit_file": "edit",
    "file_edit": "edit",
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

REASONING_TAG_PATTERN = re.compile(
    r"^<\s*(/?)\s*(analysis|reasoning|think|thinking)\s*>$",
    re.IGNORECASE,
)


class ReasoningStreamFilter:
    def __init__(self) -> None:
        self._buffer = ""
        self._depth = 0

    def feed(self, content: str, *, final: bool = False) -> str:
        source = f"{self._buffer}{content}"
        self._buffer = ""
        visible: list[str] = []
        cursor = 0

        while cursor < len(source):
            tag_start = source.find("<", cursor)
            if tag_start < 0:
                if self._depth == 0:
                    visible.append(source[cursor:])
                break
            if self._depth == 0:
                visible.append(source[cursor:tag_start])

            tag_end = source.find(">", tag_start + 1)
            if tag_end < 0:
                if final:
                    if self._depth == 0:
                        visible.append(source[tag_start:])
                else:
                    self._buffer = source[tag_start:]
                break

            tag = source[tag_start : tag_end + 1]
            match = REASONING_TAG_PATTERN.match(tag)
            if match:
                if match.group(1):
                    self._depth = max(0, self._depth - 1)
                else:
                    self._depth += 1
            elif self._depth == 0:
                visible.append(tag)
            cursor = tag_end + 1

        if final:
            self._buffer = ""
            self._depth = 0
        return "".join(visible)


def strip_reasoning_content(content: str) -> str:
    return ReasoningStreamFilter().feed(content, final=True).strip()


class AgentEventNormalizer:
    def __init__(
        self,
        *,
        run_id: str,
        trace_id: str | None,
        message_context: Mapping[str, Any] | None = None,
    ) -> None:
        self.run_id = run_id
        self.trace_id = trace_id
        self.message_context = {
            key: value
            for key, value in dict(message_context or {}).items()
            if isinstance(key, str) and value is not None
        }
        clear_file_tool_contexts()
        self._counter = 0
        self._started_tool_call_ids: set[str] = set()
        self._tool_call_context: dict[str, dict[str, Any]] = {}
        self._message_delta_buffer: list[str] = []
        self._message_completed_emitted = False
        self._latest_platform_tool_result: Any | None = None
        self._reasoning_filter = ReasoningStreamFilter()

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
        content = strip_reasoning_content(extract_completed_content(result))
        self._message_completed_emitted = True
        self._message_delta_buffer.clear()
        return self._event(
            "message.completed",
            self._message_payload(
                {
                    "run_id": self.run_id,
                    "content": content,
                    "result": sanitize_completed_result(result, content),
                }
            ),
        )

    def _dict_to_events(
        self, raw_event: dict[str, Any], *, include_messages: bool
    ) -> list[dict[str, Any]]:
        explicit_type = raw_event.get("type") or raw_event.get("event")
        if explicit_type in PLATFORM_EVENT_TYPES:
            payload = raw_event.get("payload", raw_event)
            if explicit_type == "message.completed" and isinstance(payload, dict):
                payload = sanitize_completed_payload(payload)
                payload = self._message_payload(payload)
                self._message_completed_emitted = True
                self._message_delta_buffer.clear()
            elif explicit_type == "message.delta" and isinstance(payload, dict):
                content = payload.get("content")
                if isinstance(content, str):
                    visible = self._visible_message_delta(content)
                    if not visible:
                        return []
                    payload = {**payload, "content": visible}
                    self._message_delta_buffer.append(visible)
                payload = self._message_payload(payload)
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
        for item in message_stream_items(messages):
            events.extend(self._tool_call_delta_events(item))
            events.extend(self._tool_call_started_events(item))
            tool_event = self._tool_message_to_event(item)
            if tool_event is not None:
                events.append(tool_event)
                continue
            content = extract_message_content(item)
            if content:
                visible = self._visible_message_delta(content)
                if not visible:
                    continue
                self._message_delta_buffer.append(visible)
                events.append(
                    self._event(
                        "message.delta",
                        self._message_payload(
                            {"run_id": self.run_id, "content": visible}
                        ),
                    )
                )
        return events

    def _visible_message_delta(self, content: str, *, final: bool = False) -> str:
        visible = self._reasoning_filter.feed(content, final=final)
        if not self._message_delta_buffer:
            visible = visible.lstrip()
        return visible

    def pending_completed_message(self) -> dict[str, Any] | None:
        trailing = self._visible_message_delta("", final=True)
        if trailing:
            self._message_delta_buffer.append(trailing)
        if self._message_completed_emitted or not self._message_delta_buffer:
            return None
        content = "".join(self._message_delta_buffer).strip()
        self._message_completed_emitted = True
        self._message_delta_buffer.clear()
        return self._event(
            "message.completed",
            self._message_payload(
                {
                    "run_id": self.run_id,
                    "content": content,
                    "result": {"message": content},
                }
            ),
        )

    def platform_tool_result_completed_message(self) -> dict[str, Any] | None:
        if self._message_completed_emitted or self._latest_platform_tool_result is None:
            return None
        return self.completed_message(self._latest_platform_tool_result)

    def _tool_call_delta_events(self, message: Any) -> list[dict[str, Any]]:
        events: list[dict[str, Any]] = []
        for chunk in extract_tool_call_chunks(message):
            tool_name = chunk.get("name") or self._tool_call_context.get(
                chunk["id"], {}
            ).get("tool_name")
            if not tool_name or not is_deepagents_builtin_tool(tool_name):
                continue
            arguments_delta = chunk.get("args") or ""
            if not arguments_delta:
                continue
            tool_call_id = chunk["id"]
            context = self._tool_call_context.setdefault(
                tool_call_id, {"tool_name": tool_name, "arguments_text": ""}
            )
            context["tool_name"] = tool_name
            context["arguments_text"] = (
                f"{context.get('arguments_text', '')}{arguments_delta}"
            )
            arguments_text = str(context["arguments_text"])
            arguments = normalize_tool_args(arguments_text)
            if not isinstance(arguments, str):
                context["args"] = arguments
            payload = {
                "run_id": self.run_id,
                "tool_call_id": tool_call_id,
                "tool_name": tool_name,
                "status": "running",
                "arguments_delta": arguments_delta,
                "arguments_text": arguments_text,
                "input_summary": summarize_input(arguments),
            }
            owner_payload = ownership_payload(message, chunk, context)
            context.update(owner_payload)
            payload.update(owner_payload)
            payload.update(file_tool_target_payload(tool_name, arguments))
            remember_file_tool_context_from_payload(payload)
            events.append(self._event("tool.call.delta", payload))
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
            owner_payload = ownership_payload(message, tool_call)
            self._tool_call_context[tool_call_id].update(owner_payload)
            if tool_call_id in self._started_tool_call_ids:
                continue
            self._started_tool_call_ids.add(tool_call_id)
            payload = {
                "run_id": self.run_id,
                "tool_call_id": tool_call_id,
                "tool_name": tool_name,
                "status": "running",
                "input_summary": input_summary,
            }
            payload.update(owner_payload)
            payload.update(file_tool_target_payload(tool_name, tool_call["args"]))
            remember_file_tool_context_from_payload(payload)
            events.append(
                self._event(
                    "tool.call.started",
                    payload,
                )
            )
        return events

    def _tool_message_to_event(self, message: Any) -> dict[str, Any] | None:
        if not is_tool_message(message):
            return None

        tool_call_id = str(getattr(message, "tool_call_id", "") or uuid4())
        context = self._tool_call_context.get(tool_call_id, {})
        tool_name = str(getattr(message, "name", "") or context.get("tool_name") or "")
        if not is_deepagents_builtin_tool(tool_name):
            self._latest_platform_tool_result = structured_tool_result_value(
                tool_name, extract_tool_message_content(message)
            )
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
        payload.update(ownership_payload(message, context))
        input_summary = context.get("input_summary")
        if input_summary:
            payload["input_summary"] = input_summary

        if completed:
            result_value = structured_tool_result_value(tool_name, content)
            payload.update(
                file_tool_target_payload(tool_name, context.get("args"), result_value)
            )
            payload["output_summary"] = summarize_output(result_value, None)
            views = build_tool_result_views(result_value)
            if views:
                payload["views"] = views
            return self._event("tool.call.completed", payload)

        payload.update(
            file_tool_target_payload(tool_name, context.get("args"), content)
        )
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

    def _message_payload(self, payload: dict[str, Any]) -> dict[str, Any]:
        if not self.message_context:
            return payload
        return {**self.message_context, **payload}


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


def sanitize_completed_payload(payload: dict[str, Any]) -> dict[str, Any]:
    content = payload.get("content")
    if not isinstance(content, str) and "result" in payload:
        content = extract_completed_content(payload["result"])
    if not isinstance(content, str):
        return payload
    visible = strip_reasoning_content(content)
    sanitized = {**payload, "content": visible}
    if "result" in payload:
        sanitized["result"] = sanitize_completed_result(payload["result"], visible)
    return sanitized


def ensure_completed_content(payload: dict[str, Any]) -> dict[str, Any]:
    return sanitize_completed_payload(payload)


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


class MessageWithStreamMetadata:
    def __init__(self, message: Any, metadata: Mapping[str, Any]) -> None:
        existing_metadata = raw_value(message, "metadata")
        self.message = message
        self.metadata = (
            {**metadata, **existing_metadata}
            if isinstance(existing_metadata, Mapping)
            else dict(metadata)
        )

    def __getattr__(self, name: str) -> Any:
        return getattr(self.message, name)


def message_stream_items(messages: Any) -> list[Any]:
    if (
        isinstance(messages, tuple)
        and len(messages) == 2
        and isinstance(messages[1], Mapping)
        and looks_like_message(messages[0])
    ):
        return [message_with_stream_metadata(messages[0], messages[1])]
    return list(ensure_iterable(messages))


def looks_like_message(message: Any) -> bool:
    return (
        is_tool_message(message)
        or raw_tool_calls(message) is not None
        or raw_tool_call_chunks(message) is not None
        or raw_value(message, "content") is not None
    )


def message_with_stream_metadata(message: Any, metadata: Mapping[str, Any]) -> Any:
    if isinstance(message, Mapping):
        merged = dict(message)
        existing_metadata = merged.get("metadata")
        merged["metadata"] = (
            {**metadata, **existing_metadata}
            if isinstance(existing_metadata, Mapping)
            else dict(metadata)
        )
        return merged
    return MessageWithStreamMetadata(message, metadata)


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


def extract_tool_call_chunks(message: Any) -> list[dict[str, Any]]:
    raw_chunks = raw_tool_call_chunks(message)
    chunks: list[dict[str, Any]] = []
    for index, raw_chunk in enumerate(ensure_iterable(raw_chunks)):
        chunk = normalize_tool_call_chunk(raw_chunk, index)
        if chunk is not None:
            chunks.append(chunk)
    return chunks


def raw_tool_call_chunks(message: Any) -> Any:
    if isinstance(message, dict):
        return message.get("tool_call_chunks") or message.get("toolCallChunks")
    return getattr(message, "tool_call_chunks", None)


def normalize_tool_call_chunk(raw_chunk: Any, index: int) -> dict[str, Any] | None:
    if raw_chunk is None:
        return None
    call_id = (
        raw_value(raw_chunk, "id")
        or raw_value(raw_chunk, "tool_call_id")
        or raw_value(raw_chunk, "toolCallId")
        or raw_value(raw_chunk, "index")
        or index
    )
    name = raw_value(raw_chunk, "name")
    args = raw_value(raw_chunk, "args")
    if args is None:
        args = raw_value(raw_chunk, "arguments")

    function = raw_value(raw_chunk, "function")
    if isinstance(function, Mapping):
        name = name or function.get("name")
        args = args if args is not None else function.get("arguments")

    if args is None:
        return None
    chunk = {
        "id": str(call_id),
        "name": str(name or ""),
        "args": args if isinstance(args, str) else safe_repr(args),
    }
    chunk.update(ownership_payload(raw_chunk))
    return chunk


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
    call = {
        "id": str(call_id),
        "name": str(name),
        "args": normalize_tool_args(args),
    }
    call.update(ownership_payload(raw_call))
    return call


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
    if tool_name in {"write_file", "file_write"}:
        return write_file_result_value(content)
    if tool_name in {"edit_file", "file_edit"}:
        return edit_file_result_value(content)
    if tool_name == "execute":
        return execute_result_value(content)
    return parsed if parsed is not None else content


def file_tool_target_payload(
    tool_name: str, args: Any, result_value: Any | None = None
) -> dict[str, Any]:
    operation = FILE_TOOL_OPERATIONS.get(tool_name)
    if operation is None:
        return {}
    path = extract_path(result_value) or extract_path(args)
    if not path:
        return {}
    if path.startswith("/local/"):
        source, kind = "local_mount", "local_file"
    elif path.startswith("/artifacts/"):
        source, kind = "artifact", "artifact"
    elif path.startswith("/scratch/"):
        source, kind = "scratch", "scratch_file"
    else:
        source, kind = "workspace", "workspace_file"
    return {
        "target": {
            "kind": kind,
            "path": path,
        },
        "file_effects": [
            {
                "operation": operation,
                "source": source,
                "path": path,
            }
        ],
    }


OWNER_FIELD_ALIASES = {
    "subagent_id": ("subagent_id", "subagentId"),
    "subagent_name": ("subagent_name", "subagentName"),
    "parent_tool_call_id": ("parent_tool_call_id", "parentToolCallId"),
}


def ownership_payload(*sources: Any) -> dict[str, str]:
    payload: dict[str, str] = {}
    for source in sources:
        for container in ownership_sources(source):
            for target_key, aliases in OWNER_FIELD_ALIASES.items():
                if target_key in payload:
                    continue
                for alias in aliases:
                    value = raw_value(container, alias)
                    if isinstance(value, str) and value:
                        payload[target_key] = value
                        break
    return payload


def ownership_sources(source: Any) -> list[Any]:
    if source is None:
        return []
    sources = [source]
    for key in ("metadata", "additional_kwargs", "response_metadata"):
        nested = raw_value(source, key)
        if isinstance(nested, Mapping):
            sources.append(nested)
    return sources


def remember_file_tool_context_from_payload(payload: Mapping[str, Any]) -> None:
    target = payload.get("target")
    path = target.get("path") if isinstance(target, Mapping) else None
    tool_call_id = payload.get("tool_call_id")
    tool_name = payload.get("tool_name")
    subagent_id = payload.get("subagent_id")
    subagent_name = payload.get("subagent_name")
    parent_tool_call_id = payload.get("parent_tool_call_id")
    remember_file_tool_call(
        tool_call_id=tool_call_id if isinstance(tool_call_id, str) else None,
        tool_name=tool_name if isinstance(tool_name, str) else None,
        path=path if isinstance(path, str) else None,
        operation=tool_name if isinstance(tool_name, str) else None,
        subagent_id=subagent_id if isinstance(subagent_id, str) else None,
        subagent_name=subagent_name if isinstance(subagent_name, str) else None,
        parent_tool_call_id=parent_tool_call_id
        if isinstance(parent_tool_call_id, str)
        else None,
    )


def extract_path(value: Any) -> str | None:
    if isinstance(value, Mapping):
        for path_key in ("path", "file_path", "filepath"):
            path = value.get(path_key)
            if isinstance(path, str) and path:
                return path
        for key in ("target", "file", "input"):
            nested = value.get(key)
            if isinstance(nested, Mapping):
                nested_path = extract_path(nested)
                if nested_path:
                    return nested_path
    if isinstance(value, str):
        parsed = parse_literal_content(value)
        if parsed is not None:
            return extract_path(parsed)
    return None


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


def sanitize_completed_result(value: Any, visible_content: str) -> Any:
    safe = json_safe_result(value)
    if isinstance(safe, str):
        return visible_content
    if not isinstance(safe, (dict, list)):
        return {"message": visible_content}
    return sanitize_reasoning_fields(safe)


def sanitize_reasoning_fields(value: Any, *, field_name: str | None = None) -> Any:
    if isinstance(value, str):
        if field_name in {"content", "message", "reasoning", "reasoning_content"}:
            return strip_reasoning_content(value)
        return value
    if isinstance(value, list):
        return [
            sanitize_reasoning_fields(item, field_name=field_name) for item in value
        ]
    if isinstance(value, dict):
        return {
            key: sanitize_reasoning_fields(item, field_name=key)
            for key, item in value.items()
        }
    return value


def safe_repr(value: Any) -> str:
    text = str(value)
    return text if len(text) <= 4000 else text[:4000]
