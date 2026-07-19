from __future__ import annotations

import hashlib
import time
from pathlib import PurePosixPath
from fnmatch import fnmatch
from typing import Any
from uuid import uuid4

from deepagents.backends.protocol import (
    EditResult,
    FileInfo,
    GlobResult,
    GrepMatch,
    GrepResult,
    LsResult,
    ReadResult,
    WriteResult,
)

from bibi_work_agent.clients.rust_client import RustClient
from bibi_work_agent.runtime.cancellation import RunCancelled, is_run_cancelled
from bibi_work_agent.runtime.tool_context import current_file_tool_call


ARTIFACT_DRAFT_CHUNK_CHARS = 2000
ARTIFACT_DRAFT_EVENT_BATCH_SIZE = 8
MAX_ARTIFACT_DRAFT_PREVIEW_CHARS = 80_000
SUPPORTED_PREFIXES = (
    "/workspace/",
    "/local/main/",
    "/scratch/",
    "/artifacts/",
    "/memories/",
    "/policies/",
)
SUPPORTED_ROOTS = (
    "/workspace",
    "/local/main",
    "/scratch",
    "/artifacts",
    "/memories",
    "/policies",
)
LOCAL_EXEC_TIMEOUT_MS = 15_000
CONCURRENT_ARTIFACT_READ_RETRY_SECONDS = 2.0
CONCURRENT_ARTIFACT_READ_RETRY_INTERVAL_SECONDS = 0.05


class PlatformCompositeBackend:
    """Route deepagents virtual paths to platform-controlled backends.

    Python must never resolve these paths to local filesystem paths. Workspace
    reads/writes go back to Rust for authorization and revision checks.
    """

    def __init__(
        self,
        *,
        thread_id: str | None = None,
        tenant_id: str | None = None,
        actor: dict[str, Any] | None = None,
        project_id: str | None = None,
        run_id: str | None = None,
        conversation_id: str | None = None,
        trace_id: str | None = None,
        local_main_mount_id: str | None = None,
        rust: RustClient | None = None,
    ) -> None:
        self.thread_id = thread_id
        self.tenant_id = tenant_id
        self.actor = actor or {}
        self.project_id = project_id
        self.run_id = run_id
        self.conversation_id = conversation_id
        self.trace_id = trace_id
        self.local_main_mount_id = local_main_mount_id
        self.rust = rust or RustClient()
        self._scratch: dict[str, str] = {}
        self._artifacts: dict[str, str] = {}
        self._artifact_revisions: dict[str, int] = {}

    def resolve(self, path: str) -> str:
        return self._validate_path(path)

    def ls(self, path: str) -> LsResult:
        try:
            return LsResult(entries=file_infos_from_listing(self.list_files(path)))
        except RunCancelled:
            raise
        except Exception as exc:  # noqa: BLE001 - tool boundary returns errors.
            return LsResult(error=str(exc))

    def read(
        self,
        file_path: str,
        offset: int = 0,
        limit: int = 2000,
    ) -> ReadResult:
        try:
            content = self.read_text(file_path)
        except RunCancelled:
            raise
        except Exception as exc:  # noqa: BLE001 - tool boundary returns errors.
            return ReadResult(error=str(exc))
        lines = content.splitlines(keepends=True)
        if offset or limit:
            content = "".join(lines[offset : offset + limit])
        return ReadResult(file_data={"content": content, "encoding": "utf-8"})

    def write(self, file_path: str, content: str) -> WriteResult:
        try:
            self.write_text(
                file_path,
                content,
                expected_revision=0,
                reason="deepagents write_file",
                operation="write_file",
            )
        except RunCancelled:
            raise
        except Exception as exc:  # noqa: BLE001 - tool boundary returns errors.
            return WriteResult(error=str(exc))
        return WriteResult(path=file_path)

    def edit(
        self,
        file_path: str,
        old_string: str,
        new_string: str,
        replace_all: bool = False,  # noqa: FBT001, FBT002
    ) -> EditResult:
        if old_string == new_string:
            return EditResult(error="old_string and new_string must be different")

        try:
            path = self._validate_path(file_path)
            content, revision = self._read_text_with_revision(path)
        except RunCancelled:
            raise
        except Exception as exc:  # noqa: BLE001 - tool boundary returns errors.
            return EditResult(error=str(exc))

        occurrences = content.count(old_string)
        if occurrences == 0:
            return EditResult(error="old_string not found")
        if not replace_all and occurrences > 1:
            return EditResult(error="old_string is not unique")

        updated = content.replace(old_string, new_string, -1 if replace_all else 1)
        expected_revision = revision if revision is not None else 0
        if path.startswith("/workspace/") and revision is None:
            return EditResult(error="revision unavailable for workspace edit")

        try:
            self.write_text(
                path,
                updated,
                expected_revision=expected_revision,
                reason="deepagents edit_file",
                operation="edit_file",
                previous_content=content,
            )
        except RunCancelled:
            raise
        except Exception as exc:  # noqa: BLE001 - tool boundary returns errors.
            return EditResult(error=str(exc))
        return EditResult(path=path, occurrences=occurrences if replace_all else 1)

    def glob(self, pattern: str, path: str | None = None) -> GlobResult:
        try:
            prefix = self._validate_prefix(path)
            infos = file_infos_from_listing(self.list_files(prefix))
            matches = [
                info
                for info in infos
                if not info.get("is_dir")
                and matches_glob(info["path"], pattern, prefix)
            ]
        except RunCancelled:
            raise
        except Exception as exc:  # noqa: BLE001 - tool boundary returns errors.
            return GlobResult(error=str(exc))
        return GlobResult(matches=matches)

    def grep(
        self,
        pattern: str,
        path: str | None = None,
        glob: str | None = None,
    ) -> GrepResult:
        try:
            prefix = self._validate_prefix(path)
            if prefix.startswith("/scratch/") or prefix.startswith("/artifacts/"):
                matches = self._grep_scratch(pattern, prefix=prefix, glob=glob)
            else:
                result = self.search_files(pattern, prefix=prefix)
                matches = grep_matches_from_search(result, prefix=prefix, glob=glob)
        except RunCancelled:
            raise
        except Exception as exc:  # noqa: BLE001 - tool boundary returns errors.
            return GrepResult(error=str(exc))
        return GrepResult(matches=matches)

    def read_text(self, path: str) -> str:
        path = self._validate_path(path)
        self._raise_if_cancelled()
        if path.startswith("/scratch/"):
            return self._scratch[path]
        if path.startswith("/artifacts/"):
            if path in self._artifacts:
                return self._artifacts[path]
            if self._has_persistent_file_context():
                result = self._read_artifact_file_store_with_retry(path)
                content = result.get("inline_content") or ""
                self._artifacts[path] = content
                revision = numeric_revision(result)
                if revision is not None:
                    self._artifact_revisions[path] = revision
                return content
            return self._read_in_memory_artifact_with_retry(path)
        if path.startswith("/workspace/"):
            result = self._read_file_store(path)
            return result.get("inline_content") or ""
        if path.startswith("/local/main/"):
            result = self._local_exec("read_text", virtual_path=path)
            return (
                result.get("content")
                or result.get("inline_content")
                or result.get("text")
                or ""
            )
        raise PermissionError(
            f"{path} is read-only or unavailable through this backend"
        )

    def write_text(
        self,
        path: str,
        content: str,
        *,
        expected_revision: int,
        reason: str = "agent write",
        operation: str = "write_file",
        previous_content: str | None = None,
    ) -> dict[str, Any] | None:
        path = self._validate_path(path)
        self._raise_if_cancelled()
        tool_context = current_file_tool_call(path, operation)
        draft_id = self._emit_artifact_draft_started(
            path,
            content,
            operation=operation,
            previous_content=previous_content,
        )
        try:
            if path.startswith("/scratch/"):
                self._scratch[path] = content
                result = None
            elif path.startswith("/artifacts/"):
                if self._has_persistent_file_context():
                    payload = self._file_store_payload(path)
                    payload.update(
                        {
                            "inline_content": content,
                            "expected_revision": expected_revision,
                            "reason": reason,
                            "operation": operation,
                            **tool_context_payload(tool_context),
                        }
                    )
                    self._raise_if_cancelled()
                    result = self.rust.file_write(payload)
                    self._raise_if_cancelled()
                    self._artifacts[path] = content
                    revision = numeric_revision(result)
                    if revision is not None:
                        self._artifact_revisions[path] = revision
                else:
                    self._artifacts[path] = content
                    result = None
            elif path.startswith("/workspace/"):
                payload = self._file_store_payload(path)
                payload.update(
                    {
                        "inline_content": content,
                        "expected_revision": expected_revision,
                        "reason": reason,
                        "operation": operation,
                        **tool_context_payload(tool_context),
                    }
                )
                self._raise_if_cancelled()
                result = self.rust.file_write(payload)
                self._raise_if_cancelled()
            elif path.startswith("/local/main/"):
                self._raise_if_cancelled()
                result = self._local_exec(
                    "write_text",
                    virtual_path=path,
                    content=content,
                    expected_revision=expected_revision,
                    reason=reason,
                    tool_context=tool_context,
                )
            else:
                raise PermissionError(f"{path} is read-only")
        except RunCancelled:
            raise
        except Exception as exc:
            self._emit_artifact_draft_failed(
                draft_id,
                path,
                content,
                operation=operation,
                error=str(exc),
            )
            raise

        self._raise_if_cancelled()
        self._emit_artifact_draft_completed(
            draft_id,
            path,
            content,
            operation=operation,
            result=result,
        )
        return result

    def _raise_if_cancelled(self) -> None:
        if self.run_id and is_run_cancelled(str(self.run_id)):
            raise RunCancelled(str(self.run_id))

    def list_scratch(self) -> dict[str, str]:
        return dict(self._scratch)

    def list_artifacts(self) -> dict[str, str]:
        return dict(self._artifacts)

    def list_files(self, prefix: str | None = None) -> dict[str, Any]:
        prefix = self._validate_prefix(prefix)
        self._raise_if_cancelled()
        if prefix.startswith("/scratch"):
            files = sorted(path for path in self._scratch if path.startswith(prefix))
            return {
                "files": files,
                "entries": directory_entries_for_paths(files, prefix),
            }
        if prefix.startswith("/artifacts"):
            if self._has_persistent_file_context():
                payload = self._file_store_base_payload()
                payload["prefix"] = prefix
                result = self.rust.file_list(payload)
                self._raise_if_cancelled()
                return result
            files = sorted(path for path in self._artifacts if path.startswith(prefix))
            return {
                "files": files,
                "entries": directory_entries_for_paths(files, prefix),
            }
        if prefix.startswith("/workspace"):
            payload = self._file_store_base_payload()
            payload["prefix"] = self._normalize_workspace_prefix(prefix)
            result = self.rust.file_list(payload)
            self._raise_if_cancelled()
            return result
        if prefix.startswith("/local/main"):
            return self._local_exec("list", virtual_path=prefix)
        raise PermissionError(
            f"{prefix} is read-only or unavailable through this backend"
        )

    def search_files(
        self,
        query: str,
        *,
        prefix: str | None = None,
        limit: int = 50,
    ) -> dict[str, Any]:
        if not query:
            raise ValueError("query is required")
        prefix = self._validate_prefix(prefix)
        self._raise_if_cancelled()
        if prefix.startswith("/scratch"):
            matches = {
                path: content
                for path, content in self._scratch.items()
                if path.startswith(prefix) and query in content
            }
            files = sorted(matches)
            return {
                "files": files,
                "entries": directory_entries_for_paths(files, prefix),
            }
        if prefix.startswith("/artifacts"):
            if self._has_persistent_file_context():
                payload = self._file_store_base_payload()
                payload.update(
                    {
                        "query": query,
                        "prefix": prefix,
                        "limit": limit,
                    }
                )
                result = self.rust.file_search(payload)
                self._raise_if_cancelled()
                return result
            matches = {
                path: content
                for path, content in self._artifacts.items()
                if path.startswith(prefix) and query in content
            }
            files = sorted(matches)
            return {
                "files": files,
                "entries": directory_entries_for_paths(files, prefix),
            }
        if prefix.startswith("/workspace"):
            payload = self._file_store_base_payload()
            payload.update(
                {
                    "query": query,
                    "prefix": self._normalize_workspace_prefix(prefix),
                    "limit": limit,
                }
            )
            result = self.rust.file_search(payload)
            self._raise_if_cancelled()
            return result
        if prefix.startswith("/local/main"):
            return self._local_exec(
                "search",
                virtual_path=prefix,
                query=query,
                limit=limit,
            )
        raise PermissionError(
            f"{prefix} is read-only or unavailable through this backend"
        )

    def _read_text_with_revision(self, path: str) -> tuple[str, int | None]:
        self._raise_if_cancelled()
        if path.startswith("/scratch/"):
            return self._scratch[path], None
        if path.startswith("/artifacts/"):
            if path in self._artifacts:
                return self._artifacts[path], self._artifact_revisions.get(path)
            if self._has_persistent_file_context():
                result = self._read_artifact_file_store_with_retry(path)
                content = result.get("inline_content") or ""
                revision = numeric_revision(result)
                self._artifacts[path] = content
                if revision is not None:
                    self._artifact_revisions[path] = revision
                return content, revision
            return self._read_in_memory_artifact_with_retry(path), None
        if path.startswith("/workspace/"):
            result = self._read_file_store(path)
            return result.get("inline_content") or "", numeric_revision(result)
        if path.startswith("/local/main/"):
            result = self._local_exec("read_text", virtual_path=path)
            content = (
                result.get("content")
                or result.get("inline_content")
                or result.get("text")
                or ""
            )
            return content, numeric_revision(result)
        raise PermissionError(
            f"{path} is read-only or unavailable through this backend"
        )

    def _read_file_store(self, path: str) -> dict[str, Any]:
        self._raise_if_cancelled()
        result = self.rust.file_read(self._file_store_payload(path))
        self._raise_if_cancelled()
        return result

    def _read_artifact_file_store_with_retry(self, path: str) -> dict[str, Any]:
        """Tolerate a model issuing write_file and dependent read_file in one tool batch.

        DeepAgents may execute tool calls from one model response concurrently. The Rust
        file store remains authoritative; this bounded retry only bridges the short window
        before the sibling write becomes visible and never returns synthetic content.
        """
        deadline = time.monotonic() + CONCURRENT_ARTIFACT_READ_RETRY_SECONDS
        while True:
            if path in self._artifacts:
                return {
                    "inline_content": self._artifacts[path],
                    "revision": self._artifact_revisions.get(path),
                }
            try:
                return self._read_file_store(path)
            except RunCancelled:
                raise
            except Exception:
                if time.monotonic() >= deadline:
                    raise
                self._raise_if_cancelled()
                time.sleep(CONCURRENT_ARTIFACT_READ_RETRY_INTERVAL_SECONDS)

    def _read_in_memory_artifact_with_retry(self, path: str) -> str:
        """Wait for a concurrent sibling write in runs without file-store context."""
        deadline = time.monotonic() + CONCURRENT_ARTIFACT_READ_RETRY_SECONDS
        while True:
            content = self._artifacts.get(path)
            if content is not None:
                return content
            if time.monotonic() >= deadline:
                raise KeyError(path)
            self._raise_if_cancelled()
            time.sleep(CONCURRENT_ARTIFACT_READ_RETRY_INTERVAL_SECONDS)

    def _local_exec(
        self,
        operation: str,
        *,
        virtual_path: str,
        content: str | None = None,
        expected_revision: int | None = None,
        reason: str | None = None,
        query: str | None = None,
        limit: int | None = None,
        tool_context: dict[str, Any] | None = None,
    ) -> dict[str, Any]:
        self._raise_if_cancelled()
        payload = self._local_exec_payload(virtual_path)
        payload.update(
            {
                "operation": operation,
                "virtual_path": virtual_path,
                "content": content,
                "expected_revision": expected_revision,
                "reason": reason,
                "query": query,
                "max_output_bytes": 1_048_576,
                **tool_context_payload(tool_context),
            }
        )
        if limit is not None:
            payload["limit"] = limit
        self._raise_if_cancelled()
        response = self.rust.local_exec_request(payload)
        self._raise_if_cancelled()
        status = response.get("status")
        if status != "completed":
            raise PermissionError(
                response.get("error")
                or f"local executor request did not complete: {status}"
            )
        result = response.get("result")
        if not isinstance(result, dict):
            return {}
        return result

    def _local_exec_payload(self, virtual_path: str) -> dict[str, Any]:
        if not self.tenant_id:
            raise ValueError("tenant_id is required for local mount access")
        actor_user_id = self.actor.get("user_id")
        if not actor_user_id:
            raise ValueError("actor.user_id is required for local mount access")
        actor_device_id = self.actor.get("device_id")
        if not actor_device_id:
            raise ValueError("actor.device_id is required for local mount access")
        if not self.local_main_mount_id:
            raise PermissionError(
                f"no local mount is configured for virtual path: {virtual_path}"
            )
        return {
            "tenant_id": self.tenant_id,
            "actor_user_id": actor_user_id,
            "actor_device_id": actor_device_id,
            "actor_session_id": self.actor.get("session_id"),
            "device_id": actor_device_id,
            "project_id": self.project_id,
            "run_id": self.run_id,
            "local_mount_id": self.local_main_mount_id,
            "timeout_ms": LOCAL_EXEC_TIMEOUT_MS,
        }

    def _grep_scratch(
        self, pattern: str, *, prefix: str, glob: str | None
    ) -> list[GrepMatch]:
        matches: list[GrepMatch] = []
        source = self._artifacts if prefix.startswith("/artifacts/") else self._scratch
        for path, content in sorted(source.items()):
            if not path.startswith(prefix):
                continue
            if glob and not matches_glob(path, glob, prefix):
                continue
            for line_number, line in enumerate(content.splitlines(), start=1):
                if pattern in line:
                    matches.append({"path": path, "line": line_number, "text": line})
        return matches

    def _emit_artifact_draft_started(
        self,
        path: str,
        content: str,
        *,
        operation: str,
        previous_content: str | None = None,
    ) -> str | None:
        if not self.tenant_id or not self.conversation_id:
            return None

        draft_id = f"{self.run_id or 'run'}:{uuid4()}"
        content_type, renderer = artifact_content_metadata(path)
        target = artifact_draft_target(path)
        content_bytes = content.encode("utf-8")
        preview = content[:MAX_ARTIFACT_DRAFT_PREVIEW_CHARS]
        preview_bytes = preview.encode("utf-8")
        truncated = len(preview) < len(content)
        common_payload = {
            "draft_id": draft_id,
            "run_id": self.run_id,
            "project_id": self.project_id,
            "operation": operation,
            "path": path,
            "target": target,
            "content_type": content_type,
            "renderer": renderer,
            "truncated": truncated,
            "size_bytes": len(content_bytes),
            "preview_size_bytes": len(preview_bytes),
        }
        tool_context = current_file_tool_call(path, operation)
        if tool_context:
            for key in [
                "tool_call_id",
                "tool_name",
                "args_hash",
                "subagent_id",
                "subagent_name",
                "parent_tool_call_id",
            ]:
                value = tool_context.get(key)
                if value:
                    common_payload[key] = value
        if operation == "edit_file" and previous_content is not None:
            previous_preview = previous_content[:MAX_ARTIFACT_DRAFT_PREVIEW_CHARS]
            common_payload["previous_preview"] = previous_preview
            common_payload["previous_size_bytes"] = len(previous_content.encode("utf-8"))
        self._emit_artifact_draft_events(
            [
                {
                    "event_id": f"artifact.draft.started.{draft_id}",
                    "type": "artifact.draft.started",
                    "payload": {
                        **common_payload,
                        "status": "running",
                    },
                    "trace_id": self.trace_id,
                }
            ]
        )

        byte_offset = 0
        delta_batch: list[dict[str, Any]] = []
        for index, chunk in enumerate(text_chunks(preview, ARTIFACT_DRAFT_CHUNK_CHARS)):
            chunk_bytes = chunk.encode("utf-8")
            delta_batch.append(
                {
                    "event_id": f"artifact.draft.delta.{draft_id}.{index}",
                    "type": "artifact.draft.delta",
                    "payload": {
                        **common_payload,
                        "chunk_index": index,
                        "offset_bytes": byte_offset,
                        "delta": chunk,
                    },
                    "trace_id": self.trace_id,
                }
            )
            byte_offset += len(chunk_bytes)
            if len(delta_batch) >= ARTIFACT_DRAFT_EVENT_BATCH_SIZE:
                self._emit_artifact_draft_events(delta_batch)
                delta_batch = []
        if delta_batch:
            self._emit_artifact_draft_events(delta_batch)
        return draft_id

    def _emit_artifact_draft_completed(
        self,
        draft_id: str | None,
        path: str,
        content: str,
        *,
        operation: str,
        result: dict[str, Any] | None,
    ) -> None:
        if not draft_id:
            return
        content_hash = hashlib.sha256(content.encode("utf-8")).hexdigest()
        payload: dict[str, Any] = {
            "draft_id": draft_id,
            "run_id": self.run_id,
            "project_id": self.project_id,
            "operation": operation,
            "path": path,
            "target": artifact_draft_target(path),
            "status": "completed",
            "content_hash": f"sha256:{content_hash}",
            "size_bytes": len(content.encode("utf-8")),
            "truncated": len(content) > MAX_ARTIFACT_DRAFT_PREVIEW_CHARS,
        }
        tool_context = current_file_tool_call(path, operation)
        if tool_context:
            for key in [
                "tool_call_id",
                "tool_name",
                "args_hash",
                "subagent_id",
                "subagent_name",
                "parent_tool_call_id",
            ]:
                value = tool_context.get(key)
                if value:
                    payload[key] = value
        if isinstance(result, dict):
            for source_key, target_key in [
                ("revision", "revision"),
                ("content_hash", "content_hash"),
                ("object_reference_id", "object_reference_id"),
            ]:
                value = result.get(source_key)
                if value is not None:
                    payload[target_key] = value

        self._emit_artifact_draft_events(
            [
                {
                    "event_id": f"artifact.draft.completed.{draft_id}",
                    "type": "artifact.draft.completed",
                    "payload": payload,
                    "trace_id": self.trace_id,
                }
            ]
        )

    def _emit_artifact_draft_failed(
        self,
        draft_id: str | None,
        path: str,
        content: str,
        *,
        operation: str,
        error: str,
    ) -> None:
        if not draft_id:
            return
        content_hash = hashlib.sha256(content.encode("utf-8")).hexdigest()
        payload: dict[str, Any] = {
            "draft_id": draft_id,
            "run_id": self.run_id,
            "project_id": self.project_id,
            "operation": operation,
            "path": path,
            "target": artifact_draft_target(path),
            "status": "failed",
            "content_hash": f"sha256:{content_hash}",
            "size_bytes": len(content.encode("utf-8")),
            "truncated": len(content) > MAX_ARTIFACT_DRAFT_PREVIEW_CHARS,
            "error_summary": error[:1000],
        }
        tool_context = current_file_tool_call(path, operation)
        if tool_context:
            for key in [
                "tool_call_id",
                "tool_name",
                "args_hash",
                "subagent_id",
                "subagent_name",
                "parent_tool_call_id",
            ]:
                value = tool_context.get(key)
                if value:
                    payload[key] = value
        self._emit_artifact_draft_events(
            [
                {
                    "event_id": f"artifact.draft.failed.{draft_id}",
                    "type": "artifact.draft.failed",
                    "payload": payload,
                    "trace_id": self.trace_id,
                }
            ]
        )

    def _emit_artifact_draft_events(self, events: list[dict[str, Any]]) -> None:
        if not events or not self.tenant_id or not self.conversation_id:
            return
        try:
            self.rust.emit_events(
                tenant_id=self.tenant_id,
                conversation_id=self.conversation_id,
                run_id=self.run_id,
                events=events,
            )
        except Exception:
            return

    def _has_persistent_file_context(self) -> bool:
        return bool(self.tenant_id and self.project_id and self.actor.get("user_id"))

    def _file_store_base_payload(self) -> dict[str, Any]:
        if not self.tenant_id:
            raise ValueError("tenant_id is required for platform file access")
        if not self.project_id:
            raise ValueError("project_id is required for platform file access")
        actor_user_id = self.actor.get("user_id")
        if not actor_user_id:
            raise ValueError("actor.user_id is required for platform file access")
        return {
            "tenant_id": self.tenant_id,
            "actor_user_id": actor_user_id,
            "actor_device_id": self.actor.get("device_id"),
            "actor_session_id": self.actor.get("session_id"),
            "project_id": self.project_id,
            "run_id": self.run_id,
        }

    def _file_store_payload(self, path: str) -> dict[str, Any]:
        payload = self._file_store_base_payload()
        payload["path"] = path
        return payload

    @staticmethod
    def _validate_path(path: str) -> str:
        if not path:
            raise ValueError("path is required")
        if "\x00" in path:
            raise ValueError("path contains null byte")
        if path.startswith("//"):
            raise ValueError("path may not start with //")
        if not path.startswith(SUPPORTED_PREFIXES):
            raise ValueError(f"unsupported virtual path: {path}")
        if any(part == ".." for part in path.split("/")):
            raise ValueError("path may not contain ..")
        return path

    def _validate_prefix(self, prefix: str | None) -> str:
        if prefix is None or prefix in {"", ".", "./", "/", "/.", "/./"}:
            return self._default_prefix()
        if prefix in SUPPORTED_ROOTS:
            return f"{prefix}/"
        return self._validate_path(prefix)

    def _default_prefix(self) -> str:
        if self.project_id:
            return "/workspace/"
        if self.local_main_mount_id:
            return "/local/main/"
        return "/workspace/"

    @staticmethod
    def _normalize_workspace_prefix(prefix: str) -> str:
        return "" if prefix == "/workspace/" else prefix


def directory_entries_for_paths(paths: list[str], prefix: str) -> list[dict[str, Any]]:
    root = prefix if prefix.endswith("/") else f"{prefix}/"
    directories: dict[str, set[str]] = {root: set()}
    entries: list[dict[str, Any]] = []
    for path in paths:
        if not path.startswith(root):
            continue
        current = root
        parts = path.removeprefix(root).split("/")
        for part in parts[:-1]:
            next_dir = f"{current}{part}/"
            directories.setdefault(current, set()).add(next_dir)
            directories.setdefault(next_dir, set())
            current = next_dir
        directories.setdefault(current, set()).add(path)
        entries.append(
            {
                "path": path,
                "entry_type": "file",
                "depth": path_depth(path),
                "children_count": 0,
            }
        )
    entries.extend(
        {
            "path": path,
            "entry_type": "directory",
            "depth": path_depth(path),
            "children_count": len(children),
        }
        for path, children in directories.items()
    )
    return sorted(entries, key=lambda entry: (entry["path"], entry["entry_type"]))


def path_depth(path: str) -> int:
    return len([part for part in path.strip("/").split("/") if part])


def numeric_revision(payload: dict[str, Any]) -> int | None:
    revision = payload.get("revision")
    if isinstance(revision, int):
        return revision
    if isinstance(revision, str) and revision.isdecimal():
        return int(revision)
    return None


def tool_context_payload(context: dict[str, Any] | None) -> dict[str, str]:
    if not isinstance(context, dict):
        return {}
    payload: dict[str, str] = {}
    for key in ["tool_call_id", "tool_name", "args_hash", "parent_tool_call_id"]:
        value = context.get(key)
        if isinstance(value, str) and value:
            payload[key] = value
    return payload


def text_chunks(value: str, chunk_chars: int) -> list[str]:
    if not value:
        return []
    return [value[index : index + chunk_chars] for index in range(0, len(value), chunk_chars)]


def artifact_content_metadata(path: str) -> tuple[str, str]:
    suffix = PurePosixPath(path).suffix.lower()
    if suffix in {".md", ".markdown"}:
        return "text/markdown; charset=utf-8", "markdown"
    if suffix in {".html", ".htm"}:
        return "text/html; charset=utf-8", "html"
    if suffix == ".svg":
        return "image/svg+xml", "svg"
    if suffix in {".mmd", ".mermaid"}:
        return "text/vnd.mermaid; charset=utf-8", "mermaid"
    if suffix in {".drawio", ".mxfile"}:
        return "application/vnd.jgraph.mxfile+xml; charset=utf-8", "drawio"
    if suffix == ".json":
        return "application/json; charset=utf-8", "json"
    if suffix in {".py", ".rs", ".ts", ".tsx", ".js", ".jsx", ".sql", ".yaml", ".yml"}:
        return "text/plain; charset=utf-8", suffix.removeprefix(".")
    return "text/plain; charset=utf-8", "text"


def artifact_draft_target(path: str) -> dict[str, str]:
    if path.startswith("/local/"):
        kind = "local_file"
    elif path.startswith("/artifacts/"):
        kind = "artifact"
    elif path.startswith("/scratch/"):
        kind = "scratch_file"
    else:
        kind = "workspace_file"
    return {"kind": kind, "path": path}


def file_infos_from_listing(listing: dict[str, Any]) -> list[FileInfo]:
    infos: dict[str, FileInfo] = {}
    for entry in listing.get("entries") or []:
        info = file_info_from_entry(entry)
        if info is not None:
            infos[info["path"]] = info
    for file_entry in listing.get("files") or []:
        info = file_info_from_entry(file_entry, default_is_dir=False)
        if info is not None and info["path"] not in infos:
            infos[info["path"]] = info
    return sorted(infos.values(), key=lambda info: info["path"])


def file_info_from_entry(
    entry: Any, *, default_is_dir: bool | None = None
) -> FileInfo | None:
    if isinstance(entry, str):
        return {"path": entry, "is_dir": bool(default_is_dir)}
    if not isinstance(entry, dict):
        return None
    path = entry.get("path")
    if not isinstance(path, str) or not path:
        return None
    is_dir = entry.get("is_dir")
    if not isinstance(is_dir, bool):
        entry_type = entry.get("entry_type")
        if entry_type == "directory":
            is_dir = True
        elif entry_type == "file":
            is_dir = False
        else:
            is_dir = bool(default_is_dir)
    info: FileInfo = {"path": path, "is_dir": is_dir}
    size = entry.get("size_bytes", entry.get("size"))
    if isinstance(size, int):
        info["size"] = size
    modified_at = entry.get("modified_at", entry.get("created_at"))
    if isinstance(modified_at, str):
        info["modified_at"] = modified_at
    return info


def matches_glob(path: str, pattern: str, prefix: str) -> bool:
    if pattern.startswith("/"):
        return fnmatch(path, pattern)
    root = prefix if prefix.endswith("/") else f"{prefix}/"
    relative = path.removeprefix(root)
    return fnmatch(relative, pattern) or fnmatch(path, pattern)


def grep_matches_from_search(
    result: dict[str, Any], *, prefix: str, glob: str | None
) -> list[GrepMatch]:
    matches: list[GrepMatch] = []
    for entry in result.get("matches") or result.get("files") or []:
        if isinstance(entry, str):
            path = entry
            text = ""
            line = 1
        elif isinstance(entry, dict):
            path_value = entry.get("path")
            if not isinstance(path_value, str):
                continue
            path = path_value
            text_value = entry.get("text", entry.get("snippet", ""))
            text = text_value if isinstance(text_value, str) else ""
            line_value = entry.get("line", entry.get("line_number", 1))
            line = line_value if isinstance(line_value, int) else 1
        else:
            continue
        if glob and not matches_glob(path, glob, prefix):
            continue
        matches.append({"path": path, "line": line, "text": text})
    return matches
