use axum::{
    Extension, Json,
    extract::{Path, Query, State},
};
use serde_json::json;
use uuid::Uuid;

use crate::{
    features::{
        agent_platform::{ferriskey_oidc::PlatformRequestContext, models::*},
        core::errors::AppError,
    },
    startup::AppState,
};

use super::support::*;

pub async fn list_policy_bindings(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Query(query): Query<PolicyBindingQuery>,
) -> Result<Json<Vec<PolicyBindingResponse>>, AppError> {
    let tenant_id = query
        .tenant_id
        .ok_or_else(|| AppError::InvalidInput("tenant_id is required".to_string()))?;
    ensure_tenant_member(&state.connect_pool, tenant_id, ctx.platform_user_id).await?;

    let limit = query.limit.unwrap_or(100).clamp(1, 500);
    let include_disabled = query.include_disabled.unwrap_or(false);
    let rows = sqlx::query(
        r#"
        SELECT id, tenant_id, resource_type, resource_id, action, subject_type,
               subject_id, effect, risk_level, obligations, policy_version,
               created_by_user_id, created_at, disabled_at
        FROM resource_policy_bindings
        WHERE tenant_id = $1
          AND ($2::text IS NULL OR resource_type = $2)
          AND ($3::text IS NULL OR resource_id = $3)
          AND ($4::text IS NULL OR action = $4)
          AND ($5::bool OR disabled_at IS NULL)
        ORDER BY created_at DESC
        LIMIT $6
        "#,
    )
    .bind(tenant_id)
    .bind(query.resource_type)
    .bind(query.resource_id)
    .bind(query.action)
    .bind(include_disabled)
    .bind(limit)
    .fetch_all(&state.connect_pool)
    .await?;

    let bindings = rows
        .into_iter()
        .map(policy_binding_from_row)
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(Json(bindings))
}

pub async fn create_policy_binding(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<CreatePolicyBindingRequest>,
) -> Result<Json<PolicyBindingResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    require_tenant_action(&state, &ctx, payload.tenant_id, "manage", "policy_binding").await?;

    let resource_type = normalize_identifier(&payload.resource_type, "resource_type")?;
    let resource_id = normalize_non_empty(&payload.resource_id, "resource_id")?;
    let action = normalize_identifier(&payload.action, "action")?;
    let subject_type = normalize_subject_type(&payload.subject_type)?;
    let subject_id = normalize_non_empty(&payload.subject_id, "subject_id")?;
    let effect = normalize_policy_effect(&payload.effect)?;
    let risk_level = normalize_policy_risk(payload.risk_level.as_deref().unwrap_or("low"))?;
    let obligations = payload.obligations.unwrap_or_else(|| json!({}));
    if !obligations.is_object() {
        return Err(AppError::InvalidInput(
            "obligations must be a JSON object".to_string(),
        ));
    }
    let policy_version = payload
        .policy_version
        .map(|version| normalize_non_empty(&version, "policy_version"))
        .transpose()?
        .unwrap_or_else(|| LOCAL_POLICY_VERSION.to_string());

    let row = sqlx::query(
        r#"
        INSERT INTO resource_policy_bindings (
            tenant_id, resource_type, resource_id, action, subject_type, subject_id,
            effect, risk_level, obligations, policy_version, created_by_user_id
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
        RETURNING id, tenant_id, resource_type, resource_id, action, subject_type,
                  subject_id, effect, risk_level, obligations, policy_version,
                  created_by_user_id, created_at, disabled_at
        "#,
    )
    .bind(payload.tenant_id)
    .bind(resource_type)
    .bind(resource_id)
    .bind(action)
    .bind(subject_type)
    .bind(subject_id)
    .bind(effect)
    .bind(risk_level)
    .bind(obligations)
    .bind(policy_version)
    .bind(ctx.platform_user_id)
    .fetch_one(&state.connect_pool)
    .await?;

    Ok(Json(policy_binding_from_row(row)?))
}

pub async fn disable_policy_binding(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(binding_id): Path<Uuid>,
    Json(payload): Json<DisablePolicyBindingRequest>,
) -> Result<Json<PolicyBindingResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    require_ferriskey_allow(
        &state,
        &ctx,
        payload.tenant_id,
        "disable",
        "policy_binding",
        binding_id.to_string(),
        None,
    )
    .await?;

    let row = sqlx::query(
        r#"
        UPDATE resource_policy_bindings
        SET disabled_at = COALESCE(disabled_at, CURRENT_TIMESTAMP)
        WHERE id = $1 AND tenant_id = $2
        RETURNING id, tenant_id, resource_type, resource_id, action, subject_type,
                  subject_id, effect, risk_level, obligations, policy_version,
                  created_by_user_id, created_at, disabled_at
        "#,
    )
    .bind(binding_id)
    .bind(payload.tenant_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("policy binding not found".to_string()))?;

    Ok(Json(policy_binding_from_row(row)?))
}

fn normalize_identifier(value: &str, field: &str) -> Result<String, AppError> {
    let normalized = normalize_non_empty(value, field)?;
    if normalized == "*"
        || normalized
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':'))
    {
        Ok(normalized)
    } else {
        Err(AppError::InvalidInput(format!(
            "{field} contains unsupported characters"
        )))
    }
}

fn normalize_non_empty(value: &str, field: &str) -> Result<String, AppError> {
    let normalized = value.trim();
    if normalized.is_empty() {
        Err(AppError::InvalidInput(format!("{field} is required")))
    } else {
        Ok(normalized.to_string())
    }
}

fn normalize_subject_type(value: &str) -> Result<String, AppError> {
    let normalized = value.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "user" | "role" | "relation" => Ok(normalized),
        _ => Err(AppError::InvalidInput(
            "subject_type must be user, role, or relation".to_string(),
        )),
    }
}

fn normalize_policy_effect(value: &str) -> Result<String, AppError> {
    let normalized = value.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "allow" | "deny" | "review" => Ok(normalized),
        _ => Err(AppError::InvalidInput(
            "effect must be allow, deny, or review".to_string(),
        )),
    }
}

fn normalize_policy_risk(value: &str) -> Result<String, AppError> {
    let normalized = value.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "low" | "medium" | "high" | "critical" => Ok(normalized),
        _ => Err(AppError::InvalidInput(
            "risk_level must be low, medium, high, or critical".to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_binding_normalizers_accept_supported_values() {
        assert_eq!(normalize_subject_type("Role").unwrap(), "role");
        assert_eq!(normalize_policy_effect(" REVIEW ").unwrap(), "review");
        assert_eq!(normalize_policy_risk("critical").unwrap(), "critical");
        assert_eq!(
            normalize_identifier("agent_version:published", "resource_type").unwrap(),
            "agent_version:published"
        );
    }

    #[test]
    fn policy_binding_normalizers_reject_unsafe_values() {
        assert!(normalize_subject_type("group").is_err());
        assert!(normalize_policy_effect("permit").is_err());
        assert!(normalize_policy_risk("unknown").is_err());
        assert!(normalize_identifier("agent/version", "resource_type").is_err());
        assert!(normalize_non_empty(" ", "subject_id").is_err());
    }
}
