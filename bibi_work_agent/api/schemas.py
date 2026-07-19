from __future__ import annotations

from typing import Any
from uuid import UUID

from pydantic import BaseModel, Field


class ActorRef(BaseModel):
    user_id: UUID
    device_id: UUID | None = None
    session_id: UUID | None = None
    roles: list[str] = Field(default_factory=list)


class RunDispatchRequest(BaseModel):
    tenant_id: UUID
    conversation_id: UUID
    run_id: UUID
    trace_id: str
    input: dict[str, Any] = Field(default_factory=dict)
    run_config_snapshot: dict[str, Any] = Field(default_factory=dict)


class ResumeRunRequest(BaseModel):
    tenant_id: UUID
    conversation_id: UUID | None = None
    approval_id: UUID
    trace_id: str | None = None
    input: dict[str, Any] = Field(default_factory=dict)
    run_config_snapshot: dict[str, Any] = Field(default_factory=dict)
    thread_id: str | None = None
    checkpoint_id: str | None = None
    decision_payload: dict[str, Any] = Field(default_factory=dict)


class CancelRunRequest(BaseModel):
    tenant_id: UUID
    conversation_id: UUID
    trace_id: str | None = None
    reason: str = "cancelled"


class RunEvent(BaseModel):
    event_id: str | None = None
    type: str
    payload: dict[str, Any] = Field(default_factory=dict)
    trace_id: str | None = None


class IngestRunEventsRequest(BaseModel):
    tenant_id: UUID
    conversation_id: UUID
    run_id: UUID | None
    events: list[RunEvent]


class ToolAuthorizeRequest(BaseModel):
    tenant_id: UUID
    actor: ActorRef
    conversation_id: UUID | None = None
    run_id: UUID | None = None
    trace_id: str | None = None
    tool_id: UUID | None = None
    tool_name: str
    resource: dict[str, Any] | None = None
    args_hash: str | None = None
    risk_level: str | None = None
    input_summary: str | None = None
