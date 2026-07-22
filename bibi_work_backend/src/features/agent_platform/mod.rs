pub mod audit;
pub mod audit_archiving;
pub mod audit_governance;
pub mod audit_sealing;
pub mod authz;
pub mod credential_rotation;
pub mod event_store;
pub mod ferriskey_oidc;
pub mod file_lock;
pub mod file_store;
pub mod handlers;
pub mod internal_auth;
pub mod local_runtime_queue;
pub mod mcp_discovery;
pub mod mcp_health;
pub mod mcp_http;
pub mod memory_context;
pub mod memory_ingestion;
pub mod memory_vector;
pub mod models;
pub mod process_metrics;
pub mod remote_skill;
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
    extract::DefaultBodyLimit,
    routing::{delete, get, patch, post, put},
};

use crate::startup::AppState;

use self::handlers::{
    activate_memory, api_authz_batch_check, api_authz_check, archive_memory,
    audit_hash_backfill_status_handler, audit_retention_eligibility_handler,
    backfill_audit_hash_chain_handler, batch_decide_memories, bind_agent_version,
    biwork_active_conversation_count, biwork_active_lease, biwork_add_skill_external_path,
    biwork_add_team_agent, biwork_approve_channel_pairing, biwork_auth_status, biwork_auth_user,
    biwork_cancel_conversation, biwork_cancel_team_agent_run, biwork_cancel_team_run,
    biwork_change_webui_username, biwork_channel_ingress_message,
    biwork_check_conversation_approval, biwork_check_managed_agent_health,
    biwork_check_provider_health, biwork_clone_conversation,
    biwork_confirm_conversation_confirmation, biwork_conversation_associated,
    biwork_conversation_messages, biwork_conversation_workspace, biwork_create_assistant,
    biwork_create_conversation, biwork_create_cron_job, biwork_create_custom_agent,
    biwork_create_mcp_server, biwork_create_provider, biwork_create_remote_agent,
    biwork_create_skill, biwork_create_team, biwork_cron_system_resume, biwork_delete_assistant,
    biwork_delete_assistant_rule, biwork_delete_conversation, biwork_delete_cron_job,
    biwork_delete_cron_skill, biwork_delete_custom_agent, biwork_delete_mcp_server,
    biwork_delete_provider, biwork_delete_remote_agent, biwork_delete_skill, biwork_delete_team,
    biwork_detect_provider_protocol, biwork_detect_skill_external_sources,
    biwork_detect_skill_paths, biwork_disable_channel_plugin, biwork_disable_extension,
    biwork_disable_skills_market, biwork_empty_array, biwork_enable_channel_plugin,
    biwork_enable_extension, biwork_enable_skills_market, biwork_ensure_managed_acp_tool,
    biwork_ensure_node_runtime, biwork_ensure_team_session, biwork_exchange_oidc_token,
    biwork_extension_agent_activity, biwork_extension_i18n, biwork_extension_permissions,
    biwork_extension_risk_level, biwork_fetch_model_list, biwork_fetch_provider_models,
    biwork_fs_dir, biwork_fs_image_base64, biwork_fs_list, biwork_fs_metadata, biwork_fs_read,
    biwork_fs_read_buffer, biwork_fs_write, biwork_generate_webui_qr_token,
    biwork_get_agent_mcp_capabilities, biwork_get_agent_overrides, biwork_get_assistant,
    biwork_get_channel_settings, biwork_get_client_settings, biwork_get_conversation,
    biwork_get_conversation_message, biwork_get_cron_job, biwork_get_cron_skill,
    biwork_get_oidc_config, biwork_get_remote_agent, biwork_get_skill_paths,
    biwork_get_system_user, biwork_get_team, biwork_google_subscription_status,
    biwork_hub_local_runtime_required, biwork_import_assistants, biwork_import_mcp_servers,
    biwork_import_skill, biwork_import_skill_upload, biwork_list_agents_management,
    biwork_list_assistants, biwork_list_channel_pairings, biwork_list_channel_plugins,
    biwork_list_channel_sessions, biwork_list_channel_users, biwork_list_conversation_artifacts,
    biwork_list_conversation_confirmations, biwork_list_conversations,
    biwork_list_cron_job_conversations, biwork_list_cron_jobs, biwork_list_extension_acp_adapters,
    biwork_list_extension_agents, biwork_list_extension_assistants,
    biwork_list_extension_channel_plugins, biwork_list_extension_mcp_servers,
    biwork_list_extension_settings_tabs, biwork_list_extension_skills,
    biwork_list_extension_themes, biwork_list_extension_webui, biwork_list_extensions,
    biwork_list_hub_extensions, biwork_list_mcp_servers, biwork_list_providers,
    biwork_list_remote_agents, biwork_list_route_ownership, biwork_list_skill_external_paths,
    biwork_list_skill_import_history, biwork_list_skills, biwork_list_teams,
    biwork_local_runtime_required, biwork_logout, biwork_materialize_skills_for_agent,
    biwork_mcp_agent_configs, biwork_mcp_oauth_check_status, biwork_mcp_oauth_login,
    biwork_mcp_oauth_logout, biwork_openclaw_runtime, biwork_pause_team_agent_run,
    biwork_publish_agent_mcp_capabilities, biwork_read_assistant_rule, biwork_read_builtin_rule,
    biwork_read_builtin_skill, biwork_read_skill_info, biwork_record_session_activity,
    biwork_refresh_custom_agents, biwork_reject_channel_pairing, biwork_remote_agent_handshake,
    biwork_remove_skill_external_path, biwork_remove_team_agent, biwork_rename_team,
    biwork_rename_team_agent, biwork_report_mcp_local_discovery, biwork_request_channel_pairing,
    biwork_reset_conversation, biwork_revoke_channel_user, biwork_revoke_oidc_token,
    biwork_run_cron_job, biwork_runtime_ensure, biwork_save_cron_skill, biwork_scan_skills,
    biwork_search_messages, biwork_seed_system_user_credentials, biwork_send_conversation_message,
    biwork_send_team_agent_message, biwork_send_team_message, biwork_set_agent_enabled,
    biwork_set_agent_overrides, biwork_set_assistant_state, biwork_set_channel_assistant,
    biwork_set_channel_default_model, biwork_set_config_option, biwork_set_team_session_mode,
    biwork_side_question, biwork_skill_import_limits, biwork_slash_commands,
    biwork_stop_team_session, biwork_sync_channel_settings, biwork_sync_extensions,
    biwork_system_info, biwork_team_active_lease, biwork_team_run_state,
    biwork_test_bedrock_connection, biwork_test_channel_plugin, biwork_test_custom_agent,
    biwork_test_mcp_connection, biwork_test_provider, biwork_test_remote_agent_connection,
    biwork_toggle_mcp_server, biwork_update_assistant, biwork_update_client_settings,
    biwork_update_conversation, biwork_update_conversation_artifact, biwork_update_cron_job,
    biwork_update_custom_agent, biwork_update_mcp_server, biwork_update_provider,
    biwork_update_remote_agent, biwork_webui_password_auth_unsupported, biwork_workbench_bootstrap,
    biwork_write_assistant_rule, call_mcp_tool, call_third_party_tool, cancel_agent_team_run,
    cancel_run, cancel_workflow_run, cleanup_audit_partition_handler, complete_local_exec_request,
    create_agent, create_agent_team, create_agent_team_member, create_audit_legal_hold_handler,
    create_conversation, create_device, create_llm_credential, create_llm_model_profile,
    create_llm_provider, create_local_exec_permission, create_local_mount, create_mcp_server,
    create_policy_binding, create_project, create_project_mount, create_skill, create_tenant,
    create_tool, create_workflow_design, create_workflow_run, create_workspace, decide_approval,
    disable_agent, disable_agent_version, disable_llm_model_profile, disable_llm_provider,
    disable_mcp_server, disable_mcp_tool, disable_policy_binding, disable_skill,
    disable_skill_version, disable_tool, disable_tool_version, discover_mcp_tools,
    enable_mcp_server, execute_sql_tool, file_edit, file_glob, file_list, file_lock_acquire,
    file_lock_release, file_read_body, file_read_query, file_search, file_write, get_agent,
    get_agent_team, get_agent_team_run, get_agent_version,
    get_agent_version_effective_capabilities, get_conversation_event_stream,
    get_conversation_events, get_conversation_ws, get_llm_credential_rotation_health,
    get_llm_model_profile, get_llm_provider, get_local_exec_permission,
    get_local_exec_request_status, get_mcp_server, get_mcp_tool, get_me, get_run,
    get_runtime_credential, get_skill, get_skill_version, get_tool, get_tool_version,
    get_workbench_artifact_preview, get_workbench_bootstrap, get_workbench_conversation_detail,
    get_workbench_file_diff, get_workbench_file_preview, get_workbench_files_tree,
    get_workbench_workspace_detail, get_workflow_design, get_workflow_run, get_workflow_version,
    ingest_local_exec_run_events, ingest_run_events, internal_agent_run_resume,
    internal_authz_batch_check, internal_authz_check, internal_memory_access_log,
    internal_memory_candidates, internal_memory_retrieve_for_run, internal_wait_local_exec_request,
    internal_workflow_run_tick, list_agent_teams, list_agent_versions, list_agents, list_approvals,
    list_audit_legal_holds_handler, list_conversations, list_devices,
    list_llm_credential_rotation_attempts, list_llm_credentials, list_llm_model_profiles,
    list_llm_providers, list_local_mounts, list_mcp_servers, list_mcp_tools, list_memories,
    list_policy_bindings, list_projects, list_runs, list_secret_refs, list_sessions,
    list_skill_versions, list_skills, list_tenants, list_tool_versions, list_tools,
    list_workflow_designs, list_workflow_node_runs, list_workflow_runs, list_workflow_versions,
    list_workspaces, logout_current_session, next_local_exec_request, operational_metrics,
    public_file_history, public_file_list, public_file_read, public_file_search,
    public_project_artifacts, public_tool_result_artifact_read, public_tool_result_artifact_stream,
    publish_agent_version, publish_mcp_tool, publish_outbox, publish_skill_version,
    publish_tool_version, publish_workflow_version, reject_memory,
    release_audit_legal_hold_handler, request_local_exec, revoke_device, revoke_llm_credential,
    revoke_session, rotate_llm_credential, run_stream, seal_audit_hash_chain_handler,
    search_memories, search_workbench, start_agent_team_run_stream, test_llm_model_profile,
    tool_call_authorize, update_agent, update_agent_team, update_agent_team_member,
    update_conversation, update_llm_credential_rotation_policy, update_llm_model_profile,
    update_llm_provider, update_mcp_server, update_mcp_tool, update_skill, update_tool,
    update_workflow_design, update_workspace, upsert_memory, validate_agent_version,
    validate_workflow_version, verify_audit_hash_chain_handler,
};

pub fn biwork_compat_public_router() -> Router<AppState> {
    Router::new()
        .route("/auth/status", get(biwork_auth_status))
        .route("/auth/internal/users/system", get(biwork_get_system_user))
        .route(
            "/auth/internal/users/system/credentials",
            post(biwork_seed_system_user_credentials),
        )
        .route("/auth/oidc/config", get(biwork_get_oidc_config))
        .route("/auth/oidc/token", post(biwork_exchange_oidc_token))
        .route("/auth/oidc/revoke", post(biwork_revoke_oidc_token))
        .route("/route-ownership", get(biwork_list_route_ownership))
}

pub fn biwork_compat_protected_router() -> Router<AppState> {
    Router::new()
        .route("/auth/user", get(biwork_auth_user))
        .route("/auth/logout", post(biwork_logout))
        .route(
            "/auth/session/activity",
            post(biwork_record_session_activity),
        )
        .route("/me", get(get_me))
        .route(
            "/webui/change-password",
            post(biwork_webui_password_auth_unsupported),
        )
        .route("/webui/change-username", post(biwork_change_webui_username))
        .route(
            "/webui/reset-password",
            post(biwork_webui_password_auth_unsupported),
        )
        .route(
            "/webui/generate-qr-token",
            post(biwork_generate_webui_qr_token),
        )
        .route("/system/info", get(biwork_system_info))
        .route(
            "/system/ensure-node-runtime",
            post(biwork_ensure_node_runtime),
        )
        .route(
            "/system/ensure-managed-acp-tool",
            post(biwork_ensure_managed_acp_tool),
        )
        .route(
            "/google/subscription-status",
            get(biwork_google_subscription_status),
        )
        .route(
            "/bedrock/test-connection",
            post(biwork_test_bedrock_connection),
        )
        .route("/shell/open-file", post(biwork_local_runtime_required))
        .route(
            "/shell/show-item-in-folder",
            post(biwork_local_runtime_required),
        )
        .route("/shell/open-external", post(biwork_local_runtime_required))
        .route(
            "/shell/check-tool-installed",
            post(biwork_local_runtime_required),
        )
        .route(
            "/shell/open-folder-with",
            post(biwork_local_runtime_required),
        )
        .route(
            "/settings/client",
            get(biwork_get_client_settings)
                .put(biwork_update_client_settings)
                .patch(biwork_update_client_settings),
        )
        .route("/settings", patch(biwork_update_client_settings))
        .route(
            "/assistants",
            get(biwork_list_assistants).post(biwork_create_assistant),
        )
        .route("/assistants/import", post(biwork_import_assistants))
        .route(
            "/assistants/{assistant_id}",
            get(biwork_get_assistant)
                .put(biwork_update_assistant)
                .delete(biwork_delete_assistant),
        )
        .route(
            "/assistants/{assistant_id}/state",
            patch(biwork_set_assistant_state),
        )
        .route("/agents/management", get(biwork_list_agents_management))
        .route("/agents/refresh", post(biwork_refresh_custom_agents))
        .route("/agents/custom/try-connect", post(biwork_test_custom_agent))
        .route("/agents/custom", post(biwork_create_custom_agent))
        .route(
            "/agents/custom/{agent_id}",
            put(biwork_update_custom_agent).delete(biwork_delete_custom_agent),
        )
        .route(
            "/agents/provider-health-check",
            post(biwork_check_provider_health),
        )
        .route(
            "/agents/{agent_id}/enabled",
            patch(biwork_set_agent_enabled),
        )
        .route(
            "/agents/{agent_id}/health-check",
            post(biwork_check_managed_agent_health),
        )
        .route(
            "/agents/{agent_id}/overrides",
            get(biwork_get_agent_overrides).put(biwork_set_agent_overrides),
        )
        .route(
            "/agents/{agent_id}/mcp-capabilities",
            get(biwork_get_agent_mcp_capabilities).put(biwork_publish_agent_mcp_capabilities),
        )
        .route(
            "/providers",
            get(biwork_list_providers).post(biwork_create_provider),
        )
        .route("/providers/fetch-models", post(biwork_fetch_model_list))
        .route(
            "/providers/detect-protocol",
            post(biwork_detect_provider_protocol),
        )
        .route(
            "/providers/{provider_id}",
            put(biwork_update_provider).delete(biwork_delete_provider),
        )
        .route(
            "/providers/{provider_id}/models",
            post(biwork_fetch_provider_models),
        )
        .route("/providers/{provider_id}/test", post(biwork_test_provider))
        .route("/workbench/bootstrap", get(biwork_workbench_bootstrap))
        .route("/skills", get(biwork_list_skills).post(biwork_create_skill))
        .route("/skills/builtin-rule", post(biwork_read_builtin_rule))
        .route("/skills/builtin-skill", post(biwork_read_builtin_skill))
        .route(
            "/skills/assistant-rule/read",
            post(biwork_read_assistant_rule),
        )
        .route(
            "/skills/assistant-rule/write",
            post(biwork_write_assistant_rule),
        )
        .route(
            "/skills/assistant-rule/{assistant_id}",
            delete(biwork_delete_assistant_rule),
        )
        .route(
            "/skills/materialize-for-agent",
            post(biwork_materialize_skills_for_agent),
        )
        .route("/skills/info", post(biwork_read_skill_info))
        .route("/skills/import", post(biwork_import_skill))
        .route(
            "/skills/import-upload",
            post(biwork_import_skill_upload).layer(DefaultBodyLimit::max(11 * 1024 * 1024)),
        )
        .route("/skills/scan", post(biwork_scan_skills))
        .route("/skills/detect-paths", get(biwork_detect_skill_paths))
        .route(
            "/skills/detect-external",
            get(biwork_detect_skill_external_sources),
        )
        .route(
            "/skills/import-history",
            get(biwork_list_skill_import_history),
        )
        .route("/skills/import-limits", get(biwork_skill_import_limits))
        .route("/skills/paths", get(biwork_get_skill_paths))
        .route(
            "/skills/external-paths",
            get(biwork_list_skill_external_paths)
                .post(biwork_add_skill_external_path)
                .delete(biwork_remove_skill_external_path),
        )
        .route("/skills/market/enable", post(biwork_enable_skills_market))
        .route("/skills/market/disable", post(biwork_disable_skills_market))
        .route("/skills/{skill_name}", delete(biwork_delete_skill))
        .route(
            "/mcp/servers",
            get(biwork_list_mcp_servers).post(biwork_create_mcp_server),
        )
        .route("/mcp/servers/import", post(biwork_import_mcp_servers))
        .route(
            "/mcp/servers/{server_id}",
            put(biwork_update_mcp_server).delete(biwork_delete_mcp_server),
        )
        .route(
            "/mcp/servers/{server_id}/toggle",
            post(biwork_toggle_mcp_server),
        )
        .route("/mcp/agent-configs", get(biwork_mcp_agent_configs))
        .route("/mcp/test-connection", post(biwork_test_mcp_connection))
        .route(
            "/mcp/servers/{server_id}/local-discovery",
            post(biwork_report_mcp_local_discovery),
        )
        .route(
            "/mcp/oauth/check-status",
            post(biwork_mcp_oauth_check_status),
        )
        .route("/mcp/oauth/login", post(biwork_mcp_oauth_login))
        .route("/mcp/oauth/logout", post(biwork_mcp_oauth_logout))
        .route("/mcp/oauth/authenticated", get(biwork_empty_array))
        .route(
            "/remote-agents",
            get(biwork_list_remote_agents).post(biwork_create_remote_agent),
        )
        .route(
            "/remote-agents/test-connection",
            post(biwork_test_remote_agent_connection),
        )
        .route(
            "/remote-agents/{agent_id}",
            get(biwork_get_remote_agent)
                .put(biwork_update_remote_agent)
                .delete(biwork_delete_remote_agent),
        )
        .route(
            "/remote-agents/{agent_id}/handshake",
            post(biwork_remote_agent_handshake),
        )
        .route(
            "/conversations",
            get(biwork_list_conversations).post(biwork_create_conversation),
        )
        .route("/conversations/clone", post(biwork_clone_conversation))
        .route(
            "/conversations/active-count",
            get(biwork_active_conversation_count),
        )
        .route("/messages/search", get(biwork_search_messages))
        .route(
            "/conversations/{conversation_id}",
            get(biwork_get_conversation)
                .patch(biwork_update_conversation)
                .delete(biwork_delete_conversation),
        )
        .route(
            "/conversations/{conversation_id}/reset",
            post(biwork_reset_conversation),
        )
        .route(
            "/conversations/{conversation_id}/cancel",
            post(biwork_cancel_conversation),
        )
        .route(
            "/conversations/{conversation_id}/associated",
            get(biwork_conversation_associated),
        )
        .route(
            "/conversations/{conversation_id}/messages",
            get(biwork_conversation_messages).post(biwork_send_conversation_message),
        )
        .route(
            "/conversations/{conversation_id}/messages/{message_id}",
            get(biwork_get_conversation_message),
        )
        .route(
            "/conversations/{conversation_id}/side-question",
            post(biwork_side_question),
        )
        .route(
            "/conversations/{conversation_id}/artifacts",
            get(biwork_list_conversation_artifacts),
        )
        .route(
            "/conversations/{conversation_id}/artifacts/{artifact_id}",
            patch(biwork_update_conversation_artifact),
        )
        .route(
            "/conversations/{conversation_id}/confirmations",
            get(biwork_list_conversation_confirmations),
        )
        .route(
            "/conversations/{conversation_id}/confirmations/{call_id}/confirm",
            post(biwork_confirm_conversation_confirmation),
        )
        .route(
            "/conversations/{conversation_id}/approvals/check",
            get(biwork_check_conversation_approval),
        )
        .route(
            "/conversations/{conversation_id}/runtime/ensure",
            post(biwork_runtime_ensure),
        )
        .route(
            "/conversations/{conversation_id}/config-options/{option_id}",
            put(biwork_set_config_option),
        )
        .route(
            "/conversations/{conversation_id}/openclaw/runtime",
            get(biwork_openclaw_runtime),
        )
        .route(
            "/conversations/{conversation_id}/active-lease",
            post(biwork_active_lease),
        )
        .route(
            "/conversations/{conversation_id}/slash-commands",
            get(biwork_slash_commands),
        )
        .route(
            "/conversations/{conversation_id}/workspace",
            get(biwork_conversation_workspace),
        )
        .route("/teams", get(biwork_list_teams).post(biwork_create_team))
        .route(
            "/teams/{team_id}",
            get(biwork_get_team).delete(biwork_delete_team),
        )
        .route("/teams/{team_id}/agents", post(biwork_add_team_agent))
        .route(
            "/teams/{team_id}/agents/{slot_id}",
            delete(biwork_remove_team_agent),
        )
        .route(
            "/teams/{team_id}/agents/{slot_id}/name",
            patch(biwork_rename_team_agent),
        )
        .route("/teams/{team_id}/name", patch(biwork_rename_team))
        .route(
            "/teams/{team_id}/session",
            post(biwork_ensure_team_session).delete(biwork_stop_team_session),
        )
        .route(
            "/teams/{team_id}/active-lease",
            post(biwork_team_active_lease),
        )
        .route(
            "/teams/{team_id}/session-mode",
            post(biwork_set_team_session_mode),
        )
        .route("/teams/{team_id}/run-state", get(biwork_team_run_state))
        .route("/teams/{team_id}/messages", post(biwork_send_team_message))
        .route(
            "/teams/{team_id}/agents/{slot_id}/messages",
            post(biwork_send_team_agent_message),
        )
        .route(
            "/teams/{team_id}/runs/{team_run_id}/cancel",
            post(biwork_cancel_team_run),
        )
        .route(
            "/teams/{team_id}/runs/{team_run_id}/agents/{slot_id}/cancel",
            post(biwork_cancel_team_agent_run),
        )
        .route(
            "/teams/{team_id}/runs/{team_run_id}/agents/{slot_id}/pause",
            post(biwork_pause_team_agent_run),
        )
        .route("/fs/dir", post(biwork_fs_dir))
        .route("/fs/list", post(biwork_fs_list))
        .route("/fs/read", post(biwork_fs_read))
        .route("/fs/read-buffer", post(biwork_fs_read_buffer))
        .route("/fs/image-base64", post(biwork_fs_image_base64))
        .route("/fs/write", post(biwork_fs_write))
        .route("/fs/metadata", post(biwork_fs_metadata))
        .route("/fs/browse", get(biwork_local_runtime_required))
        .route("/fs/upload", post(biwork_local_runtime_required))
        .route(
            "/fs/fetch-remote-image",
            post(biwork_local_runtime_required),
        )
        .route("/fs/temp", post(biwork_local_runtime_required))
        .route("/fs/zip", post(biwork_local_runtime_required))
        .route("/fs/zip/cancel", post(biwork_local_runtime_required))
        .route("/fs/copy", post(biwork_local_runtime_required))
        .route("/fs/remove", post(biwork_local_runtime_required))
        .route("/fs/rename", post(biwork_local_runtime_required))
        .route("/fs/watch/start", post(biwork_local_runtime_required))
        .route("/fs/watch/stop", post(biwork_local_runtime_required))
        .route("/fs/watch/stop-all", post(biwork_local_runtime_required))
        .route(
            "/fs/office-watch/start",
            post(biwork_local_runtime_required),
        )
        .route("/fs/office-watch/stop", post(biwork_local_runtime_required))
        .route("/fs/snapshot/init", post(biwork_local_runtime_required))
        .route("/fs/snapshot/compare", post(biwork_local_runtime_required))
        .route("/fs/snapshot/baseline", post(biwork_local_runtime_required))
        .route("/fs/snapshot/info", post(biwork_local_runtime_required))
        .route("/fs/snapshot/dispose", post(biwork_local_runtime_required))
        .route("/fs/snapshot/stage", post(biwork_local_runtime_required))
        .route(
            "/fs/snapshot/stage-all",
            post(biwork_local_runtime_required),
        )
        .route("/fs/snapshot/unstage", post(biwork_local_runtime_required))
        .route(
            "/fs/snapshot/unstage-all",
            post(biwork_local_runtime_required),
        )
        .route("/fs/snapshot/discard", post(biwork_local_runtime_required))
        .route("/fs/snapshot/reset", post(biwork_local_runtime_required))
        .route("/fs/snapshot/branches", post(biwork_local_runtime_required))
        .route("/preview-history/list", post(biwork_local_runtime_required))
        .route("/preview-history/save", post(biwork_local_runtime_required))
        .route(
            "/preview-history/get-content",
            post(biwork_local_runtime_required),
        )
        .route("/document/convert", post(biwork_local_runtime_required))
        .route("/ppt-preview/start", post(biwork_local_runtime_required))
        .route("/ppt-preview/stop", post(biwork_local_runtime_required))
        .route("/word-preview/start", post(biwork_local_runtime_required))
        .route("/word-preview/stop", post(biwork_local_runtime_required))
        .route("/excel-preview/start", post(biwork_local_runtime_required))
        .route("/excel-preview/stop", post(biwork_local_runtime_required))
        .route("/ppt-proxy/{port}", get(biwork_local_runtime_required))
        .route(
            "/ppt-proxy/{port}/{*asset_path}",
            get(biwork_local_runtime_required),
        )
        .route(
            "/office-watch-proxy/{port}",
            get(biwork_local_runtime_required),
        )
        .route(
            "/office-watch-proxy/{port}/{*asset_path}",
            get(biwork_local_runtime_required),
        )
        .route(
            "/cron/jobs",
            get(biwork_list_cron_jobs).post(biwork_create_cron_job),
        )
        .route(
            "/cron/jobs/{job_id}",
            get(biwork_get_cron_job)
                .put(biwork_update_cron_job)
                .delete(biwork_delete_cron_job),
        )
        .route("/cron/jobs/{job_id}/run", post(biwork_run_cron_job))
        .route(
            "/cron/internal/system-resume",
            post(biwork_cron_system_resume),
        )
        .route(
            "/cron/jobs/{job_id}/conversations",
            get(biwork_list_cron_job_conversations),
        )
        .route(
            "/cron/jobs/{job_id}/skill",
            get(biwork_get_cron_skill)
                .post(biwork_save_cron_skill)
                .delete(biwork_delete_cron_skill),
        )
        .route("/channel/plugins", get(biwork_list_channel_plugins))
        .route(
            "/channel/plugins/enable",
            post(biwork_enable_channel_plugin),
        )
        .route(
            "/channel/plugins/disable",
            post(biwork_disable_channel_plugin),
        )
        .route("/channel/plugins/test", post(biwork_test_channel_plugin))
        .route(
            "/channel/ingress/messages",
            post(biwork_channel_ingress_message),
        )
        .route("/channel/pairings", get(biwork_list_channel_pairings))
        .route(
            "/channel/pairings/request",
            post(biwork_request_channel_pairing),
        )
        .route(
            "/channel/pairings/approve",
            post(biwork_approve_channel_pairing),
        )
        .route(
            "/channel/pairings/reject",
            post(biwork_reject_channel_pairing),
        )
        .route("/channel/users", get(biwork_list_channel_users))
        .route("/channel/users/revoke", post(biwork_revoke_channel_user))
        .route("/channel/sessions", get(biwork_list_channel_sessions))
        .route(
            "/channel/settings/{platform}",
            get(biwork_get_channel_settings),
        )
        .route(
            "/channel/settings/{platform}/assistant",
            put(biwork_set_channel_assistant),
        )
        .route(
            "/channel/settings/{platform}/default-model",
            put(biwork_set_channel_default_model),
        )
        .route("/channel/settings/sync", post(biwork_sync_channel_settings))
        .route("/extensions", get(biwork_list_extensions))
        .route("/extensions/themes", get(biwork_list_extension_themes))
        .route(
            "/extensions/assistants",
            get(biwork_list_extension_assistants),
        )
        .route("/extensions/agents", get(biwork_list_extension_agents))
        .route(
            "/extensions/acp-adapters",
            get(biwork_list_extension_acp_adapters),
        )
        .route(
            "/extensions/channel-plugins",
            get(biwork_list_extension_channel_plugins),
        )
        .route(
            "/extensions/mcp-servers",
            get(biwork_list_extension_mcp_servers),
        )
        .route("/extensions/skills", get(biwork_list_extension_skills))
        .route(
            "/extensions/settings-tabs",
            get(biwork_list_extension_settings_tabs),
        )
        .route("/extensions/webui", get(biwork_list_extension_webui))
        .route(
            "/extensions/agent-activity",
            get(biwork_extension_agent_activity),
        )
        .route("/extensions/sync", post(biwork_sync_extensions))
        .route("/extensions/i18n", post(biwork_extension_i18n))
        .route("/extensions/enable", post(biwork_enable_extension))
        .route("/extensions/disable", post(biwork_disable_extension))
        .route(
            "/extensions/permissions",
            post(biwork_extension_permissions),
        )
        .route("/extensions/risk-level", post(biwork_extension_risk_level))
        .route(
            "/extensions/static/{extension_name}/{*asset_path}",
            get(biwork_local_runtime_required),
        )
        .route("/hub/extensions", get(biwork_list_hub_extensions))
        .route("/hub/install", post(biwork_hub_local_runtime_required))
        .route("/hub/uninstall", post(biwork_hub_local_runtime_required))
        .route(
            "/hub/retry-install",
            post(biwork_hub_local_runtime_required),
        )
        .route(
            "/hub/check-updates",
            post(biwork_hub_local_runtime_required),
        )
        .route("/hub/update", post(biwork_hub_local_runtime_required))
        .route("/stt", post(biwork_local_runtime_required))
        .route("/stt/stream", get(biwork_local_runtime_required))
}

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
        .route("/workbench/bootstrap", get(get_workbench_bootstrap))
        .route("/workbench/search", get(search_workbench))
        .route("/workbench/files/tree", get(get_workbench_files_tree))
        .route("/workbench/files/preview", get(get_workbench_file_preview))
        .route("/workbench/files/diff", get(get_workbench_file_diff))
        .route(
            "/workbench/artifacts/{artifact_id}/preview",
            get(get_workbench_artifact_preview),
        )
        .route(
            "/workbench/workspaces/{workspace_id}",
            get(get_workbench_workspace_detail),
        )
        .route(
            "/workbench/conversations/{conversation_id}",
            get(get_workbench_conversation_detail),
        )
        .route(
            "/audit/hash-chain:verify",
            get(verify_audit_hash_chain_handler),
        )
        .route(
            "/audit/hash-chain:seal",
            post(seal_audit_hash_chain_handler),
        )
        .route(
            "/audit/hash-chain:backfill-status",
            get(audit_hash_backfill_status_handler),
        )
        .route(
            "/audit/hash-chain:backfill",
            post(backfill_audit_hash_chain_handler),
        )
        .route(
            "/audit/legal-holds",
            get(list_audit_legal_holds_handler).post(create_audit_legal_hold_handler),
        )
        .route(
            "/audit/legal-holds/{hold_id}/release",
            post(release_audit_legal_hold_handler),
        )
        .route(
            "/audit/retention/eligibility",
            get(audit_retention_eligibility_handler),
        )
        .route(
            "/audit/retention/partitions:cleanup",
            post(cleanup_audit_partition_handler),
        )
        .route("/agents", get(list_agents).post(create_agent))
        .route("/agents/{agent_id}", get(get_agent).patch(update_agent))
        .route("/agents/{agent_id}/disable", post(disable_agent))
        .route(
            "/agent-teams",
            get(list_agent_teams).post(create_agent_team),
        )
        .route(
            "/agent-teams/{team_id}",
            get(get_agent_team).patch(update_agent_team),
        )
        .route(
            "/agent-teams/{team_id}/members",
            post(create_agent_team_member),
        )
        .route(
            "/agent-teams/{team_id}/members/{member_id}",
            patch(update_agent_team_member),
        )
        .route(
            "/agent-teams/{team_id}/runs:stream",
            post(start_agent_team_run_stream),
        )
        .route(
            "/agent-teams/{team_id}/runs/{team_run_id}",
            get(get_agent_team_run),
        )
        .route(
            "/agent-team-runs/{team_run_id}/cancel",
            post(cancel_agent_team_run),
        )
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
            "/mcp-servers/{mcp_server_id}/enable",
            post(enable_mcp_server),
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
            "/llm-credentials/{credential_id}/rotate",
            post(rotate_llm_credential),
        )
        .route(
            "/llm-credentials/{credential_id}/rotation-policy",
            post(update_llm_credential_rotation_policy),
        )
        .route(
            "/llm-credential-rotation/health",
            get(get_llm_credential_rotation_health),
        )
        .route(
            "/llm-credential-rotation/attempts",
            get(list_llm_credential_rotation_attempts),
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
        .route("/workspaces/{workspace_id}", patch(update_workspace))
        .route(
            "/workspaces/{workspace_id}/local-mounts",
            get(list_local_mounts).post(create_local_mount),
        )
        .route("/local-exec/requests/next", get(next_local_exec_request))
        .route(
            "/local-exec/requests/{request_id}/status",
            get(get_local_exec_request_status),
        )
        .route(
            "/local-exec/requests/{request_id}/complete",
            post(complete_local_exec_request),
        )
        .route(
            "/local-exec/requests/{request_id}/events",
            post(ingest_local_exec_run_events),
        )
        .route(
            "/local-exec/requests/{request_id}/permissions",
            post(create_local_exec_permission),
        )
        .route(
            "/local-exec/requests/{request_id}/permissions/{approval_id}",
            get(get_local_exec_permission),
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
            "/tool-result-artifacts/stream",
            get(public_tool_result_artifact_stream),
        )
        .route(
            "/conversations",
            get(list_conversations).post(create_conversation),
        )
        .route(
            "/conversations/{conversation_id}",
            patch(update_conversation),
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
        .route("/metrics", get(operational_metrics))
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
        .route(
            "/local-exec/requests/{request_id}/wait",
            get(internal_wait_local_exec_request),
        )
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
