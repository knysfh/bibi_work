from __future__ import annotations

import time
from concurrent.futures import ThreadPoolExecutor
from uuid import uuid4

import pytest

from bibi_work_agent.backends import platform_composite_backend as backend_module
from bibi_work_agent.backends.platform_composite_backend import PlatformCompositeBackend
from bibi_work_agent.runtime.cancellation import RunCancelled
from bibi_work_agent.runtime.tool_context import file_tool_call_context


class FakeRust:
    def __init__(self) -> None:
        self.read_payloads: list[dict] = []
        self.write_payloads: list[dict] = []
        self.list_payloads: list[dict] = []
        self.search_payloads: list[dict] = []
        self.local_exec_payloads: list[dict] = []
        self.file_contents: dict[str, str] = {}
        self.file_revisions: dict[str, int] = {}

    def file_read(self, payload: dict) -> dict:
        self.read_payloads.append(payload)
        path = payload["path"]
        if path in self.file_contents:
            return {
                "inline_content": self.file_contents[path],
                "revision": self.file_revisions[path],
            }
        return {"inline_content": "workspace content", "revision": 1}

    def file_write(self, payload: dict) -> dict:
        self.write_payloads.append(payload)
        revision = payload["expected_revision"] + 1
        self.file_contents[payload["path"]] = payload.get("inline_content") or ""
        self.file_revisions[payload["path"]] = revision
        return {"revision": revision}

    def file_list(self, payload: dict) -> dict:
        self.list_payloads.append(payload)
        return {"files": [{"path": "/workspace/a.txt"}]}

    def file_search(self, payload: dict) -> dict:
        self.search_payloads.append(payload)
        return {"files": [{"path": "/workspace/a.txt"}]}

    def local_exec_request(self, payload: dict) -> dict:
        self.local_exec_payloads.append(payload)
        if payload["operation"] == "read_text":
            return {"status": "completed", "result": {"content": "local content"}}
        if payload["operation"] == "write_text":
            return {"status": "completed", "result": {"revision": 1}}
        if payload["operation"] == "list":
            return {
                "status": "completed",
                "result": {"files": [{"path": "/local/main/a.txt"}]},
            }
        return {"status": "completed", "result": {"files": []}}


def test_backend_rejects_path_escape() -> None:
    backend = PlatformCompositeBackend(thread_id="thread")

    with pytest.raises(ValueError):
        backend.resolve("/workspace/../secret.txt")
    with pytest.raises(ValueError):
        backend.resolve("/tmp/file.txt")
    with pytest.raises(ValueError):
        backend.resolve("/home")
    with pytest.raises(ValueError):
        backend.resolve("//workspace/file.txt")


def test_scratch_backend_is_run_scoped() -> None:
    backend = PlatformCompositeBackend(thread_id="thread")

    backend.write_text("/scratch/note.txt", "hello", expected_revision=0)

    assert backend.read_text("/scratch/note.txt") == "hello"
    assert backend.list_scratch() == {"/scratch/note.txt": "hello"}


def test_local_mount_virtual_path_requires_executor_bridge() -> None:
    backend = PlatformCompositeBackend(
        thread_id="thread",
        tenant_id=str(uuid4()),
        actor={"user_id": str(uuid4()), "device_id": str(uuid4())},
    )

    assert backend.resolve("/local/main/src/lib.rs") == "/local/main/src/lib.rs"
    with pytest.raises(PermissionError, match="no local mount"):
        backend.read_text("/local/main/src/lib.rs")
    with pytest.raises(PermissionError, match="no local mount"):
        backend.write_text("/local/main/src/lib.rs", "content", expected_revision=0)


def test_local_mount_delegates_file_io_to_rust_local_executor() -> None:
    rust = FakeRust()
    tenant_id = str(uuid4())
    actor_user_id = str(uuid4())
    actor_device_id = str(uuid4())
    local_mount_id = str(uuid4())
    backend = PlatformCompositeBackend(
        thread_id="thread",
        tenant_id=tenant_id,
        actor={"user_id": actor_user_id, "device_id": actor_device_id},
        run_id=str(uuid4()),
        local_main_mount_id=local_mount_id,
        rust=rust,
    )

    assert backend.read_text("/local/main/src/lib.rs") == "local content"
    assert backend.write_text(
        "/local/main/src/lib.rs",
        "updated",
        expected_revision=0,
    ) == {"revision": 1}
    assert backend.list_files("/local/main/") == {
        "files": [{"path": "/local/main/a.txt"}]
    }

    read_payload = rust.local_exec_payloads[0]
    write_payload = rust.local_exec_payloads[1]
    assert read_payload["tenant_id"] == tenant_id
    assert read_payload["actor_user_id"] == actor_user_id
    assert read_payload["actor_device_id"] == actor_device_id
    assert read_payload["local_mount_id"] == local_mount_id
    assert read_payload["operation"] == "read_text"
    assert read_payload["virtual_path"] == "/local/main/src/lib.rs"
    assert read_payload["timeout_ms"] == 15_000
    assert write_payload["operation"] == "write_text"
    assert write_payload["content"] == "updated"


def test_local_mount_is_default_prefix_without_workspace_project() -> None:
    rust = FakeRust()
    backend = PlatformCompositeBackend(
        thread_id="thread",
        tenant_id=str(uuid4()),
        actor={"user_id": str(uuid4()), "device_id": str(uuid4())},
        run_id=str(uuid4()),
        local_main_mount_id=str(uuid4()),
        rust=rust,
    )

    assert backend.list_files() == {"files": [{"path": "/local/main/a.txt"}]}
    ls_result = backend.ls("/")
    dot_ls_result = backend.ls("/.")
    glob_result = backend.glob("*.txt")
    assert backend.list_files(".") == {"files": [{"path": "/local/main/a.txt"}]}
    assert backend.search_files("needle") == {"files": []}

    assert ls_result.error is None
    assert ls_result.entries == [{"path": "/local/main/a.txt", "is_dir": False}]
    assert dot_ls_result.error is None
    assert dot_ls_result.entries == [{"path": "/local/main/a.txt", "is_dir": False}]
    assert glob_result.error is None
    assert glob_result.matches == [{"path": "/local/main/a.txt", "is_dir": False}]
    assert [payload["operation"] for payload in rust.local_exec_payloads] == [
        "list",
        "list",
        "list",
        "list",
        "list",
        "search",
    ]
    assert all(
        payload["virtual_path"] == "/local/main/"
        for payload in rust.local_exec_payloads
    )
    assert all(payload["timeout_ms"] == 15_000 for payload in rust.local_exec_payloads)

    with pytest.raises(ValueError, match="project_id is required"):
        backend.list_files("/workspace/")


def test_artifacts_backend_is_run_scoped() -> None:
    backend = PlatformCompositeBackend(thread_id="thread")

    backend.write_text("/artifacts/report.md", "hello needle", expected_revision=0)

    assert backend.read_text("/artifacts/report.md") == "hello needle"
    assert backend.list_artifacts() == {"/artifacts/report.md": "hello needle"}
    assert backend.search_files("needle", prefix="/artifacts/")["files"] == [
        "/artifacts/report.md"
    ]


def test_artifact_read_waits_for_concurrent_sibling_write() -> None:
    class EventuallyVisibleRust(FakeRust):
        def file_read(self, payload: dict) -> dict:
            path = payload["path"]
            if path not in self.file_contents:
                raise KeyError(path)
            return super().file_read(payload)

        def file_write(self, payload: dict) -> dict:
            time.sleep(0.1)
            return super().file_write(payload)

    rust = EventuallyVisibleRust()
    backend = PlatformCompositeBackend(
        thread_id="thread",
        tenant_id=str(uuid4()),
        actor={"user_id": str(uuid4()), "device_id": str(uuid4())},
        project_id=str(uuid4()),
        run_id=str(uuid4()),
        rust=rust,
    )
    path = "/artifacts/concurrent.txt"
    with ThreadPoolExecutor(max_workers=2) as executor:
        read_future = executor.submit(backend.read_text, path)
        write_future = executor.submit(
            backend.write_text,
            path,
            "written-before-read-returns",
            expected_revision=0,
        )
        assert write_future.result(timeout=3) == {"revision": 1}
        assert read_future.result(timeout=3) == "written-before-read-returns"


def test_run_scoped_artifact_read_waits_for_concurrent_sibling_write() -> None:
    backend = PlatformCompositeBackend(thread_id="thread", run_id=str(uuid4()))
    path = "/artifacts/run-scoped-concurrent.txt"

    def delayed_write() -> dict | None:
        time.sleep(0.1)
        return backend.write_text(path, "run-scoped-content", expected_revision=0)

    with ThreadPoolExecutor(max_workers=2) as executor:
        read_future = executor.submit(backend.read_text, path)
        write_future = executor.submit(delayed_write)
        assert write_future.result(timeout=3) is None
        assert read_future.result(timeout=3) == "run-scoped-content"


def test_artifact_write_persists_to_rust_file_store_with_tool_context() -> None:
    rust = FakeRust()
    tenant_id = str(uuid4())
    project_id = str(uuid4())
    actor_user_id = str(uuid4())
    run_id = str(uuid4())
    backend = PlatformCompositeBackend(
        thread_id="thread",
        tenant_id=tenant_id,
        project_id=project_id,
        run_id=run_id,
        actor={"user_id": actor_user_id},
        rust=rust,
    )

    with file_tool_call_context(
        tool_call_id="call-artifact",
        tool_name="write_file",
        path="/artifacts/report.md",
        operation="write_file",
        args_hash="artifact-args-sha",
        parent_tool_call_id="call-task",
    ):
        result = backend.write_text(
            "/artifacts/report.md",
            "hello artifact",
            expected_revision=0,
            reason="agent generated artifact",
            operation="write_file",
        )

    assert result == {"revision": 1}
    assert backend.read_text("/artifacts/report.md") == "hello artifact"
    payload = rust.write_payloads[0]
    assert payload["tenant_id"] == tenant_id
    assert payload["project_id"] == project_id
    assert payload["actor_user_id"] == actor_user_id
    assert payload["run_id"] == run_id
    assert payload["path"] == "/artifacts/report.md"
    assert payload["inline_content"] == "hello artifact"
    assert payload["expected_revision"] == 0
    assert payload["reason"] == "agent generated artifact"
    assert payload["operation"] == "write_file"
    assert payload["tool_call_id"] == "call-artifact"
    assert payload["tool_name"] == "write_file"
    assert payload["args_hash"] == "artifact-args-sha"
    assert payload["parent_tool_call_id"] == "call-task"


def test_artifact_write_does_not_cache_when_rust_persistence_fails() -> None:
    rust = FakeRust()
    backend = PlatformCompositeBackend(
        thread_id="thread",
        tenant_id=str(uuid4()),
        project_id=str(uuid4()),
        run_id=str(uuid4()),
        actor={"user_id": str(uuid4())},
        rust=rust,
    )

    def fail_write(payload: dict) -> dict:
        rust.write_payloads.append(payload)
        raise RuntimeError("rust write failed")

    rust.file_write = fail_write  # type: ignore[method-assign]

    with pytest.raises(RuntimeError, match="rust write failed"):
        backend.write_text(
            "/artifacts/report.md",
            "not persisted",
            expected_revision=0,
            reason="agent generated artifact",
        )

    assert backend.list_artifacts() == {}


def test_artifact_edit_uses_persisted_revision() -> None:
    rust = FakeRust()
    backend = PlatformCompositeBackend(
        thread_id="thread",
        tenant_id=str(uuid4()),
        project_id=str(uuid4()),
        run_id=str(uuid4()),
        actor={"user_id": str(uuid4())},
        rust=rust,
    )

    backend.write_text(
        "/artifacts/report.md",
        "first version",
        expected_revision=0,
        reason="initial artifact",
    )
    result = backend.edit("/artifacts/report.md", "first", "second")

    assert result.error is None
    assert rust.write_payloads[1]["path"] == "/artifacts/report.md"
    assert rust.write_payloads[1]["expected_revision"] == 1
    assert rust.write_payloads[1]["reason"] == "deepagents edit_file"
    assert rust.write_payloads[1]["operation"] == "edit_file"


def test_workspace_backend_delegates_to_rust() -> None:
    rust = FakeRust()
    tenant_id = str(uuid4())
    project_id = str(uuid4())
    actor_user_id = str(uuid4())
    run_id = str(uuid4())
    backend = PlatformCompositeBackend(
        thread_id="thread",
        tenant_id=tenant_id,
        project_id=project_id,
        run_id=run_id,
        actor={"user_id": actor_user_id},
        rust=rust,
    )

    assert backend.read_text("/workspace/a.txt") == "workspace content"
    write_result = backend.write_text(
        "/workspace/a.txt",
        "updated",
        expected_revision=1,
        reason="test",
    )

    assert write_result == {"revision": 2}
    assert rust.read_payloads[0]["actor_user_id"] == actor_user_id
    assert rust.write_payloads[0]["tenant_id"] == tenant_id
    assert rust.write_payloads[0]["project_id"] == project_id
    assert rust.write_payloads[0]["run_id"] == run_id
    assert rust.write_payloads[0]["inline_content"] == "updated"


def test_workspace_write_stops_before_rust_call_when_run_cancelled(monkeypatch) -> None:
    rust = FakeRust()
    backend = PlatformCompositeBackend(
        thread_id="thread",
        tenant_id=str(uuid4()),
        project_id=str(uuid4()),
        actor={"user_id": str(uuid4())},
        run_id=str(uuid4()),
        rust=rust,
    )
    monkeypatch.setattr(backend_module, "is_run_cancelled", lambda _run_id: True)

    with pytest.raises(RunCancelled):
        backend.write("/workspace/a.txt", "updated")

    assert rust.write_payloads == []


def test_workspace_read_surfaces_stop_before_rust_when_run_cancelled(monkeypatch) -> None:
    rust = FakeRust()
    backend = PlatformCompositeBackend(
        thread_id="thread",
        tenant_id=str(uuid4()),
        project_id=str(uuid4()),
        actor={"user_id": str(uuid4())},
        run_id=str(uuid4()),
        rust=rust,
    )
    monkeypatch.setattr(backend_module, "is_run_cancelled", lambda _run_id: True)

    with pytest.raises(RunCancelled):
        backend.read_text("/workspace/a.txt")
    with pytest.raises(RunCancelled):
        backend.list_files("/workspace/")
    with pytest.raises(RunCancelled):
        backend.search_files("needle", prefix="/workspace/")
    with pytest.raises(RunCancelled):
        backend.edit("/workspace/a.txt", "old", "new")

    assert rust.read_payloads == []
    assert rust.list_payloads == []
    assert rust.search_payloads == []
    assert rust.write_payloads == []


@pytest.mark.parametrize(
    ("operation_name", "cancel_states", "payload_attr"),
    [
        ("read", [False, False, True], "read_payloads"),
        ("list", [False, True], "list_payloads"),
        ("search", [False, True], "search_payloads"),
        ("write", [False, False, True], "write_payloads"),
    ],
)
def test_workspace_backend_raises_after_rust_call_when_run_cancelled(
    monkeypatch,
    operation_name: str,
    cancel_states: list[bool],
    payload_attr: str,
) -> None:
    rust = FakeRust()
    run_id = str(uuid4())
    backend = PlatformCompositeBackend(
        thread_id="thread",
        tenant_id=str(uuid4()),
        project_id=str(uuid4()),
        actor={"user_id": str(uuid4())},
        run_id=run_id,
        rust=rust,
    )
    states = iter(cancel_states)
    monkeypatch.setattr(
        backend_module,
        "is_run_cancelled",
        lambda checked_run_id: checked_run_id == run_id and next(states, True),
    )

    with pytest.raises(RunCancelled):
        if operation_name == "read":
            backend.read_text("/workspace/a.txt")
        elif operation_name == "list":
            backend.list_files("/workspace/")
        elif operation_name == "search":
            backend.search_files("needle", prefix="/workspace/")
        elif operation_name == "write":
            backend.write_text("/workspace/a.txt", "updated", expected_revision=1)
        else:
            raise AssertionError(f"unknown operation: {operation_name}")

    assert len(getattr(rust, payload_attr)) == 1


def test_local_mount_backend_raises_after_rust_call_when_run_cancelled(
    monkeypatch,
) -> None:
    rust = FakeRust()
    run_id = str(uuid4())
    backend = PlatformCompositeBackend(
        thread_id="thread",
        tenant_id=str(uuid4()),
        actor={"user_id": str(uuid4()), "device_id": str(uuid4())},
        run_id=run_id,
        local_main_mount_id=str(uuid4()),
        rust=rust,
    )
    states = iter([False, False, False, True])
    monkeypatch.setattr(
        backend_module,
        "is_run_cancelled",
        lambda checked_run_id: checked_run_id == run_id and next(states, True),
    )

    with pytest.raises(RunCancelled):
        backend.read_text("/local/main/src/lib.rs")

    assert len(rust.local_exec_payloads) == 1


def test_workspace_write_includes_file_tool_context() -> None:
    rust = FakeRust()
    backend = PlatformCompositeBackend(
        thread_id="thread",
        tenant_id=str(uuid4()),
        project_id=str(uuid4()),
        actor={"user_id": str(uuid4())},
        run_id=str(uuid4()),
        rust=rust,
    )

    with file_tool_call_context(
        tool_call_id="call-write",
        tool_name="write_file",
        path="/workspace/a.txt",
        operation="write_file",
        args_hash="args-sha",
        parent_tool_call_id="call-task",
    ):
        backend.write_text(
            "/workspace/a.txt",
            "updated",
            expected_revision=1,
            reason="agent generated file",
            operation="write_file",
        )

    payload = rust.write_payloads[0]
    assert payload["tool_call_id"] == "call-write"
    assert payload["tool_name"] == "write_file"
    assert payload["args_hash"] == "args-sha"
    assert payload["parent_tool_call_id"] == "call-task"
    assert payload["reason"] == "agent generated file"
    assert payload["operation"] == "write_file"


def test_local_mount_write_includes_file_tool_context() -> None:
    rust = FakeRust()
    backend = PlatformCompositeBackend(
        thread_id="thread",
        tenant_id=str(uuid4()),
        actor={"user_id": str(uuid4()), "device_id": str(uuid4())},
        run_id=str(uuid4()),
        local_main_mount_id=str(uuid4()),
        rust=rust,
    )

    with file_tool_call_context(
        tool_call_id="call-local-write",
        tool_name="write_file",
        path="/local/main/a.txt",
        operation="write_file",
        args_hash="local-args-sha",
    ):
        backend.write_text(
            "/local/main/a.txt",
            "updated",
            expected_revision=2,
            reason="agent local edit",
            operation="write_file",
        )

    payload = rust.local_exec_payloads[0]
    assert payload["tool_call_id"] == "call-local-write"
    assert payload["tool_name"] == "write_file"
    assert payload["args_hash"] == "local-args-sha"
    assert payload["reason"] == "agent local edit"
    assert payload["operation"] == "write_text"
    assert payload["expected_revision"] == 2


def test_workspace_list_and_search_delegate_to_rust() -> None:
    rust = FakeRust()
    tenant_id = str(uuid4())
    project_id = str(uuid4())
    actor_user_id = str(uuid4())
    backend = PlatformCompositeBackend(
        thread_id="thread",
        tenant_id=tenant_id,
        project_id=project_id,
        actor={"user_id": actor_user_id},
        rust=rust,
    )

    assert backend.list_files("/workspace/docs/") == {
        "files": [{"path": "/workspace/a.txt"}]
    }
    assert backend.search_files("needle", prefix="/workspace/docs/", limit=10) == {
        "files": [{"path": "/workspace/a.txt"}]
    }

    assert rust.list_payloads[0]["tenant_id"] == tenant_id
    assert rust.list_payloads[0]["prefix"] == "/workspace/docs/"
    assert rust.search_payloads[0]["query"] == "needle"
    assert rust.search_payloads[0]["limit"] == 10


def test_deepagents_ls_protocol_lists_workspace_entries() -> None:
    rust = FakeRust()
    backend = PlatformCompositeBackend(
        thread_id="thread",
        tenant_id=str(uuid4()),
        project_id=str(uuid4()),
        actor={"user_id": str(uuid4())},
        rust=rust,
    )

    result = backend.ls("/workspace/")

    assert result.error is None
    assert result.entries == [{"path": "/workspace/a.txt", "is_dir": False}]
    assert rust.list_payloads[0]["prefix"] == ""


def test_deepagents_read_write_and_edit_protocol_delegate_to_workspace() -> None:
    rust = FakeRust()
    backend = PlatformCompositeBackend(
        thread_id="thread",
        tenant_id=str(uuid4()),
        project_id=str(uuid4()),
        actor={"user_id": str(uuid4())},
        rust=rust,
    )

    read_result = backend.read("/workspace/a.txt")
    write_result = backend.write("/workspace/new.md", "new content")
    edit_result = backend.edit("/workspace/a.txt", "workspace", "updated")

    assert read_result.error is None
    assert read_result.file_data == {
        "content": "workspace content",
        "encoding": "utf-8",
    }
    assert write_result.error is None
    assert write_result.path == "/workspace/new.md"
    assert edit_result.error is None
    assert edit_result.occurrences == 1
    assert rust.write_payloads[0]["path"] == "/workspace/new.md"
    assert rust.write_payloads[0]["expected_revision"] == 0
    assert rust.write_payloads[1]["path"] == "/workspace/a.txt"
    assert rust.write_payloads[1]["expected_revision"] == 1
    assert rust.write_payloads[1]["inline_content"] == "updated content"


def test_scratch_list_and_search_are_run_scoped() -> None:
    backend = PlatformCompositeBackend(thread_id="thread")

    backend.write_text("/scratch/a.txt", "hello needle", expected_revision=0)
    backend.write_text("/scratch/docs/c.txt", "nested needle", expected_revision=0)
    backend.write_text("/scratch/b.txt", "hello", expected_revision=0)

    listed = backend.list_files("/scratch/")
    assert listed["files"] == [
        "/scratch/a.txt",
        "/scratch/b.txt",
        "/scratch/docs/c.txt",
    ]
    assert {
        (entry["path"], entry["entry_type"], entry["children_count"])
        for entry in listed["entries"]
    } >= {
        ("/scratch/", "directory", 3),
        ("/scratch/docs/", "directory", 1),
        ("/scratch/a.txt", "file", 0),
    }

    searched = backend.search_files("needle", prefix="/scratch/")
    assert searched["files"] == ["/scratch/a.txt", "/scratch/docs/c.txt"]
    assert {(entry["path"], entry["entry_type"]) for entry in searched["entries"]} >= {
        ("/scratch/", "directory"),
        ("/scratch/docs/", "directory"),
        ("/scratch/a.txt", "file"),
        ("/scratch/docs/c.txt", "file"),
    }


def test_deepagents_grep_and_glob_protocol_use_scratch_files() -> None:
    backend = PlatformCompositeBackend(thread_id="thread")
    backend.write_text("/scratch/a.txt", "hello needle", expected_revision=0)
    backend.write_text("/scratch/docs/c.md", "nested needle", expected_revision=0)
    backend.write_text("/scratch/b.txt", "hello", expected_revision=0)

    grep_result = backend.grep("needle", path="/scratch/", glob="*.txt")
    glob_result = backend.glob("**/*.md", path="/scratch/")

    assert grep_result.error is None
    assert grep_result.matches == [
        {"path": "/scratch/a.txt", "line": 1, "text": "hello needle"}
    ]
    assert glob_result.error is None
    assert glob_result.matches == [{"path": "/scratch/docs/c.md", "is_dir": False}]
