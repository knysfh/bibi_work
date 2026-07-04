from __future__ import annotations

from uuid import uuid4

from bibi_work_agent.runtime import run_executor


class FakeRust:
    def __init__(self) -> None:
        self.emitted_batches: list[dict] = []

    def emit_events(self, **kwargs):
        self.emitted_batches.append(kwargs)
        return {}


class WorkflowNodeAgent:
    def stream(self, input_payload, config=None):  # noqa: ARG002
        node_key = input_payload["node_key"]
        upstream_outputs = input_payload.get("upstream_outputs", {})
        if node_key == "a":
            value = "a:seed"
        elif node_key == "b":
            value = f"b:{upstream_outputs['a']['value']}"
        else:
            value = f"c:{upstream_outputs['b']['value']}"
        yield {
            "type": "message.completed",
            "payload": {
                "run_id": input_payload["run_id"],
                "result": {
                    "node_key": node_key,
                    "value": value,
                    "upstream_outputs": upstream_outputs,
                },
            },
        }


def test_runtime_executes_three_workflow_node_payloads_in_dag_order(
    monkeypatch,
) -> None:
    fake_rust = FakeRust()
    monkeypatch.setattr(run_executor, "RustClient", lambda: fake_rust)
    monkeypatch.setattr(run_executor, "is_run_cancelled", lambda _run_id: False)
    monkeypatch.setattr(
        run_executor, "create_platform_agent", lambda _snapshot: WorkflowNodeAgent()
    )

    tenant_id = str(uuid4())
    conversation_id = str(uuid4())
    workflow_run_id = str(uuid4())
    outputs: dict[str, dict] = {}

    for node_key in ("a", "b", "c"):
        run_id = str(uuid4())
        payload = {
            "tenant_id": tenant_id,
            "conversation_id": conversation_id,
            "run_id": run_id,
            "trace_id": f"trace-{node_key}",
            "thread_id": f"workflow:{workflow_run_id}:node:{node_key}:attempt:1",
            "input": {
                "run_id": run_id,
                "workflow_run_id": workflow_run_id,
                "node_key": node_key,
                "workflow_input": {"seed": True},
                "upstream_outputs": dict(outputs),
            },
            "run_config_snapshot": {
                "tenant_id": tenant_id,
                "conversation_id": conversation_id,
                "run_id": run_id,
                "thread_id": f"workflow:{workflow_run_id}:node:{node_key}:attempt:1",
                "actor": {"user_id": str(uuid4())},
                "workflow": {
                    "workflow_run_id": workflow_run_id,
                    "node_key": node_key,
                },
                "agent": {"model": "fake-model", "system_prompt": "DAG node"},
            },
        }
        run_executor.execute_run_payload(f"task-{node_key}", payload)
        outputs[node_key] = latest_message_result(fake_rust)

    assert outputs["a"]["value"] == "a:seed"
    assert outputs["b"]["value"] == "b:a:seed"
    assert outputs["c"]["value"] == "c:b:a:seed"

    event_types = [
        event["type"]
        for batch in fake_rust.emitted_batches
        for event in batch["events"]
    ]
    assert event_types == [
        "run.started",
        "message.completed",
        "run.completed",
        "run.started",
        "message.completed",
        "run.completed",
        "run.started",
        "message.completed",
        "run.completed",
    ]


def latest_message_result(fake_rust: FakeRust) -> dict:
    for batch in reversed(fake_rust.emitted_batches):
        for event in reversed(batch["events"]):
            if event["type"] == "message.completed":
                return event["payload"]["result"]
    raise AssertionError("message.completed event was not emitted")
