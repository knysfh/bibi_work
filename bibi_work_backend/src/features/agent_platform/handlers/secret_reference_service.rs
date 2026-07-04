use axum::{
    Extension, Json,
    extract::{Query, State},
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    features::{
        agent_platform::{ferriskey_oidc::PlatformRequestContext, secret_resolver},
        core::errors::AppError,
    },
    startup::AppState,
};

use super::support::ensure_tenant_member;

#[derive(Debug, Deserialize)]
pub struct SecretRefListQuery {
    pub tenant_id: Option<Uuid>,
    pub purpose: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SecretRefResponse {
    pub id: String,
    pub label: String,
    pub purpose: String,
    pub scheme: String,
    pub available: bool,
}

pub async fn list_secret_refs(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Query(query): Query<SecretRefListQuery>,
) -> Result<Json<Vec<SecretRefResponse>>, AppError> {
    let tenant_id = query
        .tenant_id
        .ok_or_else(|| AppError::InvalidInput("tenant_id is required".to_string()))?;
    ensure_tenant_member(&state.connect_pool, tenant_id, ctx.platform_user_id).await?;
    let requested_purpose = query.purpose.as_deref().unwrap_or("all");
    let refs = std::env::var("BIBI_WORK_SECRET_REFS")
        .unwrap_or_default()
        .split(',')
        .filter_map(parse_secret_ref_entry)
        .filter(|item| {
            requested_purpose == "all"
                || item.purpose.as_str() == requested_purpose
                || item.purpose == "general"
        })
        .collect::<Vec<_>>();
    Ok(Json(refs))
}

fn parse_secret_ref_entry(raw: &str) -> Option<SecretRefResponse> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let (label_part, secret_ref) = raw.split_once('=').unwrap_or((raw, raw));
    let (purpose, label) = label_part
        .split_once(':')
        .unwrap_or(("general", label_part));
    let secret_ref = secret_ref.trim();
    let scheme = secret_ref
        .split_once("://")
        .map(|(scheme, _)| scheme)
        .unwrap_or("unknown")
        .to_string();
    Some(SecretRefResponse {
        id: secret_ref.to_string(),
        label: label.trim().to_string(),
        purpose: purpose.trim().to_string(),
        scheme,
        available: secret_available(secret_ref),
    })
}

fn secret_available(secret_ref: &str) -> bool {
    secret_resolver::env_name_from_secret_ref(secret_ref)
        .ok()
        .and_then(|name| std::env::var(name).ok())
        .is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_secret_ref_allowlist_without_secret_values() {
        unsafe {
            std::env::set_var("BIBI_TEST_LISTED_SECRET", "hidden");
        }
        let parsed = parse_secret_ref_entry("llm:OpenAI=env://BIBI_TEST_LISTED_SECRET")
            .expect("entry parsed");

        assert_eq!(parsed.id, "env://BIBI_TEST_LISTED_SECRET");
        assert_eq!(parsed.label, "OpenAI");
        assert_eq!(parsed.purpose, "llm");
        assert_eq!(parsed.scheme, "env");
        assert!(parsed.available);
    }
}
