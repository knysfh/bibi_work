use serde_json::Value;

use super::biwork_compat_service::value_string;

pub(super) const BIWORK_ACTIVE_LEASE_SECONDS: i64 = 90;

pub(super) fn runtime_kind(config: &Value, metadata: &Value) -> String {
    value_string(config, "acp_backend")
        .or_else(|| {
            config
                .pointer("/runtime/kind")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .or_else(|| value_string(metadata, "runtime_kind"))
        .unwrap_or_else(|| "deepagents".to_string())
}

pub(super) fn biwork_agent_type(runtime: &str, metadata: &Value) -> String {
    if value_string(metadata, "source")
        .map(|source| source.eq_ignore_ascii_case("remote"))
        .unwrap_or(false)
    {
        return "remote".to_string();
    }

    match runtime.trim().to_ascii_lowercase().as_str() {
        "remote" => "remote".to_string(),
        "openclaw-gateway" => "openclaw-gateway".to_string(),
        "nanobot" => "nanobot".to_string(),
        _ => "acp".to_string(),
    }
}

pub(super) fn biwork_assistant_runtime_disabled_reason(
    runtime: &str,
    agent_type: &str,
) -> Option<String> {
    let runtime = runtime.trim().to_ascii_lowercase();
    let agent_type = agent_type.trim().to_ascii_lowercase();
    if agent_type == "remote" || runtime == "remote" {
        return Some(
            "remote agent runtime is hidden because direct remote-agent execution is not adapted in this release"
                .to_string(),
        );
    }

    match runtime.as_str() {
        "deepagents" => None,
        "biwork_cli" => None,
        "acp" => Some(format!(
            "runtime.kind={runtime} is obsolete; configure the agent with runtime.kind=biwork_cli"
        )),
        "disabled" => Some("runtime.kind=disabled is catalog-visible but not runnable".to_string()),
        other => Some(format!(
            "runtime.kind={other} is not supported by Rust compat dispatch"
        )),
    }
}

pub(super) fn normalize_biwork_agent_source(source: Option<&str>) -> String {
    let Some(source) = source.map(str::trim).filter(|value| !value.is_empty()) else {
        return "internal".to_string();
    };

    match source.to_ascii_lowercase().as_str() {
        "internal" => "internal".to_string(),
        "builtin" => "builtin".to_string(),
        "extension" => "extension".to_string(),
        _ => "custom".to_string(),
    }
}
