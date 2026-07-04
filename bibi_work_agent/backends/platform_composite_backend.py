from __future__ import annotations

from fnmatch import fnmatch
from typing import Any

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
        local_main_mount_id: str | None = None,
        rust: RustClient | None = None,
    ) -> None:
        self.thread_id = thread_id
        self.tenant_id = tenant_id
        self.actor = actor or {}
        self.project_id = project_id
        self.run_id = run_id
        self.local_main_mount_id = local_main_mount_id
        self.rust = rust or RustClient()
        self._scratch: dict[str, str] = {}
        self._artifacts: dict[str, str] = {}

    def resolve(self, path: str) -> str:
        return self._validate_path(path)

    def ls(self, path: str) -> LsResult:
        try:
            return LsResult(entries=file_infos_from_listing(self.list_files(path)))
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
            )
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
            )
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
        except Exception as exc:  # noqa: BLE001 - tool boundary returns errors.
            return GrepResult(error=str(exc))
        return GrepResult(matches=matches)

    def read_text(self, path: str) -> str:
        path = self._validate_path(path)
        if path.startswith("/scratch/"):
            return self._scratch[path]
        if path.startswith("/artifacts/"):
            return self._artifacts[path]
        if path.startswith("/workspace/"):
            result = self._read_workspace(path)
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
    ) -> dict[str, Any] | None:
        path = self._validate_path(path)
        if path.startswith("/scratch/"):
            self._scratch[path] = content
            return None
        if path.startswith("/artifacts/"):
            self._artifacts[path] = content
            return None
        if path.startswith("/workspace/"):
            payload = self._workspace_payload(path)
            payload.update(
                {
                    "inline_content": content,
                    "expected_revision": expected_revision,
                    "reason": reason,
                }
            )
            return self.rust.file_write(payload)
        if path.startswith("/local/main/"):
            return self._local_exec(
                "write_text",
                virtual_path=path,
                content=content,
                expected_revision=expected_revision,
            )
        raise PermissionError(f"{path} is read-only")

    def list_scratch(self) -> dict[str, str]:
        return dict(self._scratch)

    def list_artifacts(self) -> dict[str, str]:
        return dict(self._artifacts)

    def list_files(self, prefix: str | None = None) -> dict[str, Any]:
        prefix = self._validate_prefix(prefix)
        if prefix.startswith("/scratch"):
            files = sorted(path for path in self._scratch if path.startswith(prefix))
            return {
                "files": files,
                "entries": directory_entries_for_paths(files, prefix),
            }
        if prefix.startswith("/artifacts"):
            files = sorted(path for path in self._artifacts if path.startswith(prefix))
            return {
                "files": files,
                "entries": directory_entries_for_paths(files, prefix),
            }
        if prefix.startswith("/workspace"):
            payload = self._workspace_base_payload()
            payload["prefix"] = self._normalize_workspace_prefix(prefix)
            return self.rust.file_list(payload)
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
            payload = self._workspace_base_payload()
            payload.update(
                {
                    "query": query,
                    "prefix": self._normalize_workspace_prefix(prefix),
                    "limit": limit,
                }
            )
            return self.rust.file_search(payload)
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
        if path.startswith("/scratch/"):
            return self._scratch[path], None
        if path.startswith("/artifacts/"):
            return self._artifacts[path], None
        if path.startswith("/workspace/"):
            result = self._read_workspace(path)
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

    def _read_workspace(self, path: str) -> dict[str, Any]:
        return self.rust.file_read(self._workspace_payload(path))

    def _local_exec(
        self,
        operation: str,
        *,
        virtual_path: str,
        content: str | None = None,
        expected_revision: int | None = None,
        query: str | None = None,
        limit: int | None = None,
    ) -> dict[str, Any]:
        payload = self._local_exec_payload(virtual_path)
        payload.update(
            {
                "operation": operation,
                "virtual_path": virtual_path,
                "content": content,
                "expected_revision": expected_revision,
                "query": query,
                "max_output_bytes": 1_048_576,
            }
        )
        if limit is not None:
            payload["limit"] = limit
        response = self.rust.local_exec_request(payload)
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

    def _workspace_base_payload(self) -> dict[str, Any]:
        if not self.tenant_id:
            raise ValueError("tenant_id is required for workspace file access")
        if not self.project_id:
            raise ValueError("project_id is required for workspace file access")
        actor_user_id = self.actor.get("user_id")
        if not actor_user_id:
            raise ValueError("actor.user_id is required for workspace file access")
        return {
            "tenant_id": self.tenant_id,
            "actor_user_id": actor_user_id,
            "actor_device_id": self.actor.get("device_id"),
            "actor_session_id": self.actor.get("session_id"),
            "project_id": self.project_id,
            "run_id": self.run_id,
        }

    def _workspace_payload(self, path: str) -> dict[str, Any]:
        payload = self._workspace_base_payload()
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
