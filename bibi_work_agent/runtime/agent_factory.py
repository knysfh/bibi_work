from __future__ import annotations

from typing import Any

from bibi_work_agent.api.schemas import ActorRef
from bibi_work_agent.backends.platform_composite_backend import PlatformCompositeBackend
from bibi_work_agent.clients.rust_client import RustClient
from bibi_work_agent.runtime.checkpointer import PlatformCheckpointer
from bibi_work_agent.runtime.memory_retrieval import append_memory_context
from bibi_work_agent.tools.platform_adapters import PlatformToolAdapters
from bibi_work_agent.tools.result_presenter import normalize_ui_hints
from bibi_work_agent.tools.wrapper import PlatformToolWrapper


def create_platform_agent(snapshot: dict[str, Any]) -> Any:
    try:
        from deepagents import create_deep_agent  # type: ignore
    except Exception as exc:  # noqa: BLE001
        raise RuntimeError("deepagents is not installed or cannot be imported") from exc

    agent_snapshot = snapshot.get("agent", {})
    model = snapshot.get("model") or agent_snapshot.get("model")
    if not model:
        raise RuntimeError("run_config_snapshot.agent.model is required")
    model = build_runtime_chat_model(resolve_runtime_model_credentials(model, snapshot))

    thread_id = snapshot.get("thread_id")
    backend = PlatformCompositeBackend(
        thread_id=thread_id,
        tenant_id=snapshot.get("tenant_id"),
        actor=snapshot.get("actor"),
        project_id=snapshot.get("project_id"),
        run_id=snapshot.get("run_id"),
        local_main_mount_id=local_mount_id_for(snapshot, "/local/main/"),
    )
    checkpointer = snapshot.get("checkpointer")
    if checkpointer is None and thread_id:
        checkpointer = PlatformCheckpointer(
            thread_id=thread_id,
            tenant_id=snapshot.get("tenant_id"),
        )
    tools = build_platform_tools(snapshot, backend=backend)

    return create_deep_agent(
        model=model,
        system_prompt=build_system_prompt(snapshot),
        tools=tools,
        subagents=snapshot.get("subagents") or agent_snapshot.get("subagents", []),
        backend=backend,
        permissions=snapshot.get("permissions"),
        interrupt_on=snapshot.get("interrupt_on"),
        checkpointer=checkpointer,
    )


def resolve_runtime_model_credentials(
    model: Any,
    snapshot: dict[str, Any],
    *,
    rust: RustClient | None = None,
) -> Any:
    if not isinstance(model, dict):
        return model

    resolved_model = dict(model)
    credential = model.get("credential")
    if not isinstance(credential, dict):
        return resolved_model

    runtime_credential_id = credential.get("runtime_credential_id")
    if not runtime_credential_id:
        return resolved_model

    tenant_id = snapshot.get("tenant_id")
    run_id = snapshot.get("run_id")
    if not tenant_id or not run_id:
        raise RuntimeError(
            "tenant_id and run_id are required to resolve runtime credential"
        )

    payload = (rust or RustClient()).runtime_credential(
        tenant_id=tenant_id,
        run_id=run_id,
        runtime_credential_id=str(runtime_credential_id),
    )
    auth_scheme = str(
        payload.get("auth_scheme") or model.get("auth_scheme") or "bearer"
    )
    if auth_scheme not in {"bearer", "api_key_header", "none"}:
        raise RuntimeError(f"unsupported llm auth_scheme: {auth_scheme}")

    if auth_scheme != "none":
        resolved_model["api_key"] = payload["secret"]
    if "model" not in resolved_model and resolved_model.get("model_name"):
        resolved_model["model"] = resolved_model["model_name"]
    resolved_model["credential"] = {
        key: value
        for key, value in credential.items()
        if key not in {"secret", "secret_ref", "api_key", "token", "password"}
    }
    return resolved_model


def build_runtime_chat_model(model: Any) -> Any:
    if not isinstance(model, dict):
        return model

    provider = str(model.get("provider") or "").replace("_", "-").lower()
    if provider not in {"openai", "openai-compatible"}:
        raise RuntimeError(f"unsupported llm provider: {provider or '<missing>'}")

    model_name = str(model.get("model") or model.get("model_name") or "").strip()
    if not model_name:
        raise RuntimeError("llm model_name is required")

    try:
        from langchain.chat_models import init_chat_model
    except Exception as exc:  # noqa: BLE001
        raise RuntimeError("langchain chat model support is not available") from exc

    kwargs: dict[str, Any] = {"model_provider": "openai"}
    if model.get("api_key"):
        kwargs["api_key"] = model["api_key"]
    if model.get("base_url"):
        kwargs["base_url"] = model["base_url"]

    parameters = model.get("parameters")
    if isinstance(parameters, dict):
        for source_key, target_key in [
            ("temperature", "temperature"),
            ("top_p", "top_p"),
            ("max_output_tokens", "max_completion_tokens"),
            ("reasoning_effort", "reasoning_effort"),
        ]:
            if parameters.get(source_key) is not None:
                kwargs[target_key] = parameters[source_key]

    return init_chat_model(model_name, **kwargs)


def local_mount_id_for(snapshot: dict[str, Any], virtual_path: str) -> str | None:
    workspace = snapshot.get("workspace")
    if not isinstance(workspace, dict):
        return None
    local_mounts = workspace.get("local_mounts")
    if not isinstance(local_mounts, list):
        return None
    for mount in local_mounts:
        if not isinstance(mount, dict):
            continue
        if local_mount_virtual_path_matches(mount.get("virtual_path"), virtual_path):
            mount_id = mount.get("local_mount_id") or mount.get("id")
            return str(mount_id) if mount_id else None
    return None


def local_mount_virtual_path_matches(candidate: Any, expected: str) -> bool:
    if candidate == expected:
        return True
    return expected == "/local/main/" and candidate == "/local/"


def build_system_prompt(snapshot: dict[str, Any]) -> str:
    agent_snapshot = snapshot.get("agent", {})
    system_prompt = snapshot.get("system_prompt") or agent_snapshot.get(
        "system_prompt", ""
    )
    system_prompt = append_workspace_filesystem_context(system_prompt, snapshot)
    memory_context = snapshot.get("memory_context") or agent_snapshot.get(
        "memory_context"
    )
    return append_memory_context(system_prompt, memory_context)


def append_workspace_filesystem_context(prompt: str, snapshot: dict[str, Any]) -> str:
    context = workspace_filesystem_context(snapshot)
    if not context:
        return prompt
    if prompt.strip():
        return f"{prompt.rstrip()}\n\n{context}"
    return context


def workspace_filesystem_context(snapshot: dict[str, Any]) -> str:
    workspace = snapshot.get("workspace")
    if not isinstance(workspace, dict):
        workspace = {}

    project_id = snapshot.get("project_id") or workspace.get("remote_project_id")
    local_roots = [
        str(mount["virtual_path"])
        for mount in workspace.get("local_mounts") or []
        if isinstance(mount, dict) and mount.get("virtual_path")
    ]
    has_local_main = any(
        local_mount_virtual_path_matches(root, "/local/main/") for root in local_roots
    )

    lines = ["Platform file access uses virtual paths only."]
    if project_id:
        lines.append("Remote project files root: /workspace/.")
    if has_local_main:
        lines.append(
            "Mounted local folder root: /local/main/. Use it as the current workspace directory for local folder requests."
        )
    if not project_id and has_local_main:
        lines.append(
            "The /workspace/ root is unavailable because this run has no project_id."
        )
    if len(lines) == 1:
        return ""
    lines.append("Do not use real OS paths such as /home or /Users.")
    return "\n".join(lines)


def build_platform_tools(
    snapshot: dict[str, Any],
    *,
    backend: PlatformCompositeBackend,
    rust: RustClient | None = None,
) -> list[Any]:
    specs = snapshot.get("tools") or snapshot.get("agent", {}).get("tools", [])
    if not specs:
        return []

    wrapper = PlatformToolWrapper(
        rust=rust or RustClient(),
        tenant_id=snapshot["tenant_id"],
        actor=ActorRef(**snapshot["actor"]),
        conversation_id=snapshot.get("conversation_id"),
        run_id=snapshot.get("run_id"),
        project_id=snapshot.get("project_id"),
        trace_id=snapshot.get("trace_id"),
    )
    adapters = PlatformToolAdapters(
        rust=wrapper.rust,
        tenant_id=snapshot["tenant_id"],
        actor=wrapper.actor,
        conversation_id=snapshot.get("conversation_id"),
        run_id=snapshot.get("run_id"),
        project_id=snapshot.get("project_id"),
        backend=backend,
    )

    tools: list[Any] = []
    for spec in specs:
        tool_name = tool_spec_name(spec)
        func = platform_tool_callable(tool_name, adapters)
        tools.append(wrapper.wrap(tool_name, func, ui_hints=tool_spec_ui_hints(spec)))
    return tools


def tool_spec_name(spec: Any) -> str:
    if isinstance(spec, str):
        return spec
    if isinstance(spec, dict):
        name = spec.get("name") or spec.get("tool_name")
        if isinstance(name, str) and name:
            return name
    raise RuntimeError("platform tool spec requires a name")


def tool_spec_ui_hints(spec: Any) -> dict[str, Any] | None:
    if not isinstance(spec, dict):
        return None
    for source in [
        spec,
        spec.get("schema"),
        spec.get("output_schema"),
        spec.get("metadata"),
    ]:
        if not isinstance(source, dict):
            continue
        hints = normalize_ui_hints(source)
        if hints:
            return hints
        output_schema = source.get("output_schema")
        if isinstance(output_schema, dict):
            hints = normalize_ui_hints(output_schema)
            if hints:
                return hints
    return None


def platform_tool_callable(tool_name: str, adapters: PlatformToolAdapters) -> Any:
    return adapters.callable_for(tool_name)
