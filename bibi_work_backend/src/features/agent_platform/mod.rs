pub mod audit;
pub mod audit_sealing;
pub mod authz;
pub mod event_store;
pub mod ferriskey_oidc;
pub mod file_lock;
pub mod file_store;
pub mod handlers;
pub mod internal_auth;
pub mod mcp_discovery;
pub mod memory_context;
pub mod memory_ingestion;
pub mod memory_vector;
pub mod models;
pub mod run_lifecycle;
pub mod run_snapshot;
pub mod runtime;
pub mod rustfs;
pub mod secret_resolver;
pub mod tool_execution;
pub mod workflow_compile;
pub mod workflow_mapping;
pub mod workflow_plan;
pub mod workflow_runtime;

use axum::{
    Router,
    routing::{get, post},
};

use crate::startup::AppState;

use self::handlers::{
    activate_memory, api_authz_batch_check, api_authz_check, archive_memory, batch_decide_memories,
    bind_agent_version, call_mcp_tool, call_third_party_tool, cancel_run, cancel_workflow_run,
    complete_local_exec_request, create_agent, create_conversation, create_device,
    create_llm_credential, create_llm_model_profile, create_llm_provider, create_local_mount,
    create_mcp_server, create_policy_binding, create_project, create_project_mount, create_skill,
    create_tenant, create_tool, create_workflow_design, create_workflow_run, create_workspace,
    decide_approval, disable_agent, disable_agent_version, disable_llm_model_profile,
    disable_llm_provider, disable_mcp_server, disable_mcp_tool, disable_policy_binding,
    disable_skill, disable_skill_version, disable_tool, disable_tool_version, discover_mcp_tools,
    execute_sql_tool, file_edit, file_glob, file_list, file_lock_acquire, file_lock_release,
    file_read_body, file_read_query, file_search, file_write, get_agent, get_agent_version,
    get_agent_version_effective_capabilities, get_conversation_event_stream,
    get_conversation_events, get_conversation_ws, get_llm_model_profile, get_llm_provider,
    get_mcp_server, get_mcp_tool, get_me, get_run, get_runtime_credential, get_skill,
    get_skill_version, get_tool, get_tool_version, get_workflow_design, get_workflow_run,
    get_workflow_version, ingest_run_events, internal_agent_run_resume, internal_authz_batch_check,
    internal_authz_check, internal_memory_access_log, internal_memory_candidates,
    internal_memory_retrieve_for_run, internal_workflow_run_tick, list_agent_versions, list_agents,
    list_approvals, list_conversations, list_devices, list_llm_credentials,
    list_llm_model_profiles, list_llm_providers, list_local_mounts, list_mcp_servers,
    list_mcp_tools, list_memories, list_policy_bindings, list_projects, list_runs,
    list_secret_refs, list_sessions, list_skill_versions, list_skills, list_tenants,
    list_tool_versions, list_tools, list_workflow_designs, list_workflow_node_runs,
    list_workflow_runs, list_workflow_versions, list_workspaces, logout_current_session,
    next_local_exec_request, public_file_history, public_file_list, public_file_read,
    public_file_search, public_project_artifacts, public_tool_result_artifact_read,
    publish_agent_version, publish_mcp_tool, publish_outbox, publish_skill_version,
    publish_tool_version, publish_workflow_version, reject_memory, request_local_exec,
    revoke_device, revoke_llm_credential, revoke_session, run_stream,
    seal_audit_hash_chain_handler, search_memories, test_llm_model_profile, tool_call_authorize,
    update_agent, update_llm_model_profile, update_llm_provider, update_mcp_server,
    update_mcp_tool, update_skill, update_tool, update_workflow_design, upsert_memory,
    validate_agent_version, validate_workflow_version, verify_audit_hash_chain_handler,
};

pub fn api_router() -> Router<AppState> {
    Router::new()
        .route("/me", get(get_me))
        .route("/tenants", get(list_tenants).post(create_tenant))
        .route("/devices", get(list_devices).post(create_device))
        .route("/devices/{device_id}/revoke", post(revoke_device))
        .route("/sessions", get(list_sessions))
        .route("/sessions/{session_id}/revoke", post(revoke_session))
        .route("/auth/logout", post(logout_current_session))
        .route("/secret-refs", get(list_secret_refs))
        .route("/authz/check", post(api_authz_check))
        .route("/authz/batch-check", post(api_authz_batch_check))
        .route(
            "/audit/hash-chain:verify",
            get(verify_audit_hash_chain_handler),
        )
        .route(
            "/audit/hash-chain:seal",
            post(seal_audit_hash_chain_handler),
        )
        .route("/agents", get(list_agents).post(create_agent))
        .route("/agents/{agent_id}", get(get_agent).patch(update_agent))
        .route("/agents/{agent_id}/disable", post(disable_agent))
        .route(
            "/agents/{agent_id}/versions",
            get(list_agent_versions).post(publish_agent_version),
        )
        .route("/agent-versions/{agent_version_id}", get(get_agent_version))
        .route(
            "/agent-versions/{agent_version_id}/disable",
            post(disable_agent_version),
        )
        .route(
            "/agent-versions/{agent_version_id}/bindings",
            post(bind_agent_version),
        )
        .route(
            "/agent-versions/{agent_version_id}/effective-capabilities",
            get(get_agent_version_effective_capabilities),
        )
        .route(
            "/agent-versions/{agent_version_id}/validate",
            post(validate_agent_version),
        )
        .route("/skills", get(list_skills).post(create_skill))
        .route("/skills/{skill_id}", get(get_skill).patch(update_skill))
        .route("/skills/{skill_id}/disable", post(disable_skill))
        .route(
            "/skills/{skill_id}/versions",
            get(list_skill_versions).post(publish_skill_version),
        )
        .route("/skill-versions/{skill_version_id}", get(get_skill_version))
        .route(
            "/skill-versions/{skill_version_id}/disable",
            post(disable_skill_version),
        )
        .route("/tools", get(list_tools).post(create_tool))
        .route("/tools/{tool_id}", get(get_tool).patch(update_tool))
        .route("/tools/{tool_id}/disable", post(disable_tool))
        .route(
            "/tools/{tool_id}/versions",
            get(list_tool_versions).post(publish_tool_version),
        )
        .route("/tool-versions/{tool_version_id}", get(get_tool_version))
        .route(
            "/tool-versions/{tool_version_id}/disable",
            post(disable_tool_version),
        )
        .route(
            "/mcp-servers",
            get(list_mcp_servers).post(create_mcp_server),
        )
        .route(
            "/mcp-servers/{mcp_server_id}",
            get(get_mcp_server).patch(update_mcp_server),
        )
        .route(
            "/mcp-servers/{mcp_server_id}/disable",
            post(disable_mcp_server),
        )
        .route(
            "/mcp-servers/{mcp_server_id}/tools",
            get(list_mcp_tools).post(publish_mcp_tool),
        )
        .route(
            "/mcp-servers/{mcp_server_id}/tools:discover",
            post(discover_mcp_tools),
        )
        .route(
            "/mcp-tools/{mcp_tool_id}",
            get(get_mcp_tool).patch(update_mcp_tool),
        )
        .route("/mcp-tools/{mcp_tool_id}/disable", post(disable_mcp_tool))
        .route(
            "/llm-providers",
            get(list_llm_providers).post(create_llm_provider),
        )
        .route(
            "/llm-providers/{provider_id}",
            get(get_llm_provider).patch(update_llm_provider),
        )
        .route(
            "/llm-providers/{provider_id}/disable",
            post(disable_llm_provider),
        )
        .route(
            "/llm-credentials",
            get(list_llm_credentials).post(create_llm_credential),
        )
        .route(
            "/llm-credentials/{credential_id}/revoke",
            post(revoke_llm_credential),
        )
        .route(
            "/llm-model-profiles",
            get(list_llm_model_profiles).post(create_llm_model_profile),
        )
        .route(
            "/llm-model-profiles/{profile_id}",
            get(get_llm_model_profile).patch(update_llm_model_profile),
        )
        .route(
            "/llm-model-profiles/{profile_id}/disable",
            post(disable_llm_model_profile),
        )
        .route(
            "/llm-model-profiles/{profile_id}/test",
            post(test_llm_model_profile),
        )
        .route(
            "/policy-bindings",
            get(list_policy_bindings).post(create_policy_binding),
        )
        .route(
            "/policy-bindings/{binding_id}/disable",
            post(disable_policy_binding),
        )
        .route("/projects", get(list_projects).post(create_project))
        .route("/projects/{project_id}/mounts", post(create_project_mount))
        .route("/workspaces", get(list_workspaces).post(create_workspace))
        .route(
            "/workspaces/{workspace_id}/local-mounts",
            get(list_local_mounts).post(create_local_mount),
        )
        .route("/local-exec/requests/next", get(next_local_exec_request))
        .route(
            "/local-exec/requests/{request_id}/complete",
            post(complete_local_exec_request),
        )
        .route("/projects/{project_id}/files", get(public_file_list))
        .route("/projects/{project_id}/files/read", get(public_file_read))
        .route(
            "/projects/{project_id}/files:search",
            post(public_file_search),
        )
        .route(
            "/projects/{project_id}/files/history",
            get(public_file_history),
        )
        .route(
            "/projects/{project_id}/artifacts",
            get(public_project_artifacts),
        )
        .route(
            "/tool-result-artifacts/read",
            get(public_tool_result_artifact_read),
        )
        .route(
            "/conversations",
            get(list_conversations).post(create_conversation),
        )
        .route(
            "/conversations/{conversation_id}/runs:stream",
            post(run_stream),
        )
        .route(
            "/conversations/{conversation_id}/events",
            get(get_conversation_events),
        )
        .route(
            "/conversations/{conversation_id}/events/stream",
            get(get_conversation_event_stream),
        )
        .route(
            "/conversations/{conversation_id}/ws",
            get(get_conversation_ws),
        )
        .route("/runs", get(list_runs))
        .route("/runs/{run_id}", get(get_run))
        .route("/runs/{run_id}/cancel", post(cancel_run))
        .route("/approvals", get(list_approvals))
        .route("/approvals/{approval_id}/decision", post(decide_approval))
        .route("/memories:search", post(search_memories))
        .route("/memories:batch-decision", post(batch_decide_memories))
        .route("/memories", get(list_memories).post(upsert_memory))
        .route("/memories/{memory_id}/activate", post(activate_memory))
        .route("/memories/{memory_id}/reject", post(reject_memory))
        .route("/memories/{memory_id}/archive", post(archive_memory))
        .route(
            "/workflow-designs",
            get(list_workflow_designs).post(create_workflow_design),
        )
        .route(
            "/workflow-designs/{workflow_design_id}",
            get(get_workflow_design).patch(update_workflow_design),
        )
        .route(
            "/workflow-designs/{workflow_design_id}/versions",
            get(list_workflow_versions).post(publish_workflow_version),
        )
        .route(
            "/workflow-versions/{workflow_version_id}",
            get(get_workflow_version),
        )
        .route(
            "/workflow-versions/{workflow_version_id}/validate",
            post(validate_workflow_version),
        )
        .route(
            "/workflow-runs",
            get(list_workflow_runs).post(create_workflow_run),
        )
        .route("/workflow-runs/{workflow_run_id}", get(get_workflow_run))
        .route(
            "/workflow-runs/{workflow_run_id}/node-runs",
            get(list_workflow_node_runs),
        )
        .route(
            "/workflow-runs/{workflow_run_id}/cancel",
            post(cancel_workflow_run),
        )
}

pub fn internal_router() -> Router<AppState> {
    Router::new()
        .route("/authz/check", post(internal_authz_check))
        .route("/authz/batch-check", post(internal_authz_batch_check))
        .route("/run-events", post(ingest_run_events))
        .route("/outbox/publish", post(publish_outbox))
        .route("/tool-calls:authorize", post(tool_call_authorize))
        .route(
            "/agent-runs/{run_id}/resume",
            post(internal_agent_run_resume),
        )
        .route("/files/read", get(file_read_query).post(file_read_body))
        .route("/files/write", post(file_write))
        .route("/files/edit", post(file_edit))
        .route("/files/locks/acquire", post(file_lock_acquire))
        .route("/files/locks/release", post(file_lock_release))
        .route("/files/list", get(file_list))
        .route("/files/glob", get(file_glob))
        .route("/files/search", post(file_search))
        .route(
            "/memory/retrieve-for-run",
            post(internal_memory_retrieve_for_run),
        )
        .route("/memory/candidates", post(internal_memory_candidates))
        .route("/memory/access-log", post(internal_memory_access_log))
        .route("/local-exec/requests", post(request_local_exec))
        .route("/mcp-tools:call", post(call_mcp_tool))
        .route("/sql-tools:execute", post(execute_sql_tool))
        .route("/third-party-tools:call", post(call_third_party_tool))
        .route(
            "/runtime-credentials/{runtime_credential_id}",
            get(get_runtime_credential),
        )
        .route(
            "/workflow-runs/{workflow_run_id}/tick",
            post(internal_workflow_run_tick),
        )
}
