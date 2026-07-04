from __future__ import annotations

from typing import Any

from bibi_work_agent.clients.rust_client import RustClient


class EventEmitter:
    def __init__(
        self,
        *,
        rust: RustClient,
        tenant_id: str,
        conversation_id: str,
        run_id: str | None,
    ) -> None:
        self.rust = rust
        self.tenant_id = tenant_id
        self.conversation_id = conversation_id
        self.run_id = run_id

    def emit(self, events: list[dict[str, Any]]) -> None:
        if not events:
            return
        self.rust.emit_events(
            tenant_id=self.tenant_id,
            conversation_id=self.conversation_id,
            run_id=self.run_id,
            events=events,
        )
