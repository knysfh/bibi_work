from __future__ import annotations

from uuid import uuid4

import pytest

from bibi_work_agent.backends.platform_composite_backend import PlatformCompositeBackend


class FakeRust:
    def __init__(self) -> None:
        self.read_payloads: list[dict] = []
        self.write_payloads: list[dict] = []
        self.list_payloads: list[dict] = []
        self.search_payloads: list[dict] = []
        self.local_exec_payloads: list[dict] = []

    def file_read(self, payload: dict) -> dict:
        self.read_payloads.append(payload)
        return {"inline_content": "workspace content", "revision": 1}

    def file_write(self, payload: dict) -> dict:
        self.write_payloads.append(payload)
        return {"revision": payload["expected_revision"] + 1}

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
