use axum::{Extension, Json, extract::State};
use serde_json::{Value, json};

use crate::{
    features::{agent_platform::ferriskey_oidc::PlatformRequestContext, core::errors::AppError},
    startup::AppState,
};

use super::{
    biwork_assistant_service::biwork_list_assistants, biwork_compat_service::ok,
    biwork_custom_agent_service::biwork_list_agents_management,
    biwork_mcp_service::biwork_list_mcp_servers, biwork_provider_service::biwork_list_providers,
    biwork_route_ownership_service::route_ownership_manifest,
    biwork_skill_service::biwork_list_skills,
};

fn ok_data(response: Json<Value>) -> Value {
    response.0.get("data").cloned().unwrap_or(Value::Null)
}

fn biwork_workbench_feature_flags() -> Value {
    json!({
        "auth": {
            "oidc_required": true,
            "password_login": false,
        },
        "runtime": {
            "deepagents": true,
            "biwork_cli": false,
            "disabled": true,
            "remote_agent_direct": false,
        },
        "desktop": {
            "gateway_required_for_local_capabilities": true,
            "shell": true,
            "office_preview": true,
            "preview_history": true,
            "local_remote_control": false,
            "cdp_remote_control": false,
        },
        "enterprise": {
            "assistants": true,
            "providers": true,
            "skills": true,
            "mcp": true,
            "conversations": true,
            "teams": true,
            "cron": true,
            "channel_governance": true,
            "extension_governance": true,
        },
    })
}

pub async fn biwork_workbench_bootstrap(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
) -> Result<Json<Value>, AppError> {
    let assistants =
        ok_data(biwork_list_assistants(State(state.clone()), Extension(ctx.clone())).await?);
    let providers =
        ok_data(biwork_list_providers(State(state.clone()), Extension(ctx.clone())).await?);
    let skills = ok_data(biwork_list_skills(State(state.clone()), Extension(ctx.clone())).await?);
    let mcp_servers =
        ok_data(biwork_list_mcp_servers(State(state.clone()), Extension(ctx.clone())).await?);
    let managed_agents =
        ok_data(biwork_list_agents_management(State(state), Extension(ctx.clone())).await?);
    let feature_flags = biwork_workbench_feature_flags();
    let route_ownership = route_ownership_manifest();

    Ok(ok(json!({
        "auth": {
            "tenant_id": ctx.tenant_id,
            "user_id": ctx.platform_user_id,
            "session_id": ctx.session_id,
            "device_id": ctx.device_id,
            "preferred_username": ctx.preferred_username,
            "email": ctx.email,
            "roles": ctx.roles,
        },
        "runtime": {
            "default_kind": "deepagents",
            "supported_kinds": ["deepagents", "biwork_cli", "disabled"],
            "python_runtime_kind": "deepagents",
            "desktop_runtime_kind": "biwork_cli",
        },
        "feature_flags": feature_flags,
        "route_ownership": route_ownership,
        "catalog": {
            "assistants": assistants,
            "providers": providers,
            "skills": skills,
            "mcp_servers": mcp_servers,
            "managed_agents": managed_agents,
        },
        "assistants": assistants,
        "providers": providers,
        "skills": skills,
        "mcp_servers": mcp_servers,
        "managed_agents": managed_agents,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workbench_bootstrap_feature_flags_hide_out_of_scope_capabilities() {
        let flags = biwork_workbench_feature_flags();

        assert_eq!(flags["auth"]["oidc_required"], true);
        assert_eq!(flags["auth"]["password_login"], false);
        assert_eq!(flags["runtime"]["deepagents"], true);
        assert_eq!(flags["runtime"]["remote_agent_direct"], false);
        assert_eq!(flags["desktop"]["cdp_remote_control"], false);
        assert_eq!(flags["enterprise"]["conversations"], true);
        assert_eq!(flags["enterprise"]["cron"], true);
    }
}
