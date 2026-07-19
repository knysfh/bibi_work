use axum::{
    Extension, Json,
    extract::{Query, State},
};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use sqlx::Row;
use uuid::Uuid;

use crate::{
    features::{
        agent_platform::{ferriskey_oidc::PlatformRequestContext, models::AuthzContext},
        core::errors::AppError,
    },
    startup::AppState,
};

use super::{biwork_compat_service::ok, support::require_ferriskey_allow};

#[derive(Debug, Deserialize)]
pub struct ClientSettingsQuery {
    keys: Option<String>,
}

pub async fn biwork_get_client_settings(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Query(query): Query<ClientSettingsQuery>,
) -> Result<Json<Value>, AppError> {
    let keys = query.keys.as_deref().map(parse_keys).unwrap_or_default();

    let rows = if keys.is_empty() {
        sqlx::query(
            r#"
            SELECT key, value
            FROM user_ui_preferences
            WHERE tenant_id = $1 AND user_id = $2
            ORDER BY key
            "#,
        )
        .bind(ctx.tenant_id)
        .bind(ctx.platform_user_id)
        .fetch_all(&state.connect_pool)
        .await?
    } else {
        sqlx::query(
            r#"
            SELECT key, value
            FROM user_ui_preferences
            WHERE tenant_id = $1 AND user_id = $2 AND key = ANY($3)
            ORDER BY key
            "#,
        )
        .bind(ctx.tenant_id)
        .bind(ctx.platform_user_id)
        .bind(&keys)
        .fetch_all(&state.connect_pool)
        .await?
    };

    let mut data = Map::new();
    for row in rows {
        data.insert(row.try_get::<String, _>("key")?, row.try_get("value")?);
    }
    Ok(ok(Value::Object(data)))
}

pub async fn biwork_update_client_settings(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let Some(entries) = payload.as_object() else {
        return Err(AppError::InvalidInput(
            "settings payload must be an object".to_string(),
        ));
    };
    require_biwork_user_settings_update(&state, &ctx).await?;

    let mut updated = Vec::new();
    let mut tx = state.connect_pool.begin().await?;
    for (key, value) in entries {
        if key.trim().is_empty() {
            return Err(AppError::InvalidInput("settings key is empty".to_string()));
        }
        if value.is_null() {
            sqlx::query(
                r#"
                DELETE FROM user_ui_preferences
                WHERE tenant_id = $1 AND user_id = $2 AND key = $3
                "#,
            )
            .bind(ctx.tenant_id)
            .bind(ctx.platform_user_id)
            .bind(key)
            .execute(&mut *tx)
            .await?;
        } else {
            sqlx::query(
                r#"
                INSERT INTO user_ui_preferences (tenant_id, user_id, key, value)
                VALUES ($1, $2, $3, $4)
                ON CONFLICT (tenant_id, user_id, key)
                DO UPDATE SET value = EXCLUDED.value, updated_at = CURRENT_TIMESTAMP
                "#,
            )
            .bind(ctx.tenant_id)
            .bind(ctx.platform_user_id)
            .bind(key)
            .bind(value)
            .execute(&mut *tx)
            .await?;
        }
        updated.push(key.clone());
    }
    tx.commit()
        .await
        .map_err(|_| AppError::DatabaseTransaction)?;

    Ok(ok(json!({ "updated": updated })))
}

pub(super) async fn set_biwork_client_setting(
    state: &AppState,
    tenant_id: Uuid,
    user_id: Uuid,
    key: &str,
    value: &Value,
) -> Result<(), AppError> {
    if key.trim().is_empty() {
        return Err(AppError::InvalidInput("settings key is empty".to_string()));
    }
    sqlx::query(
        r#"
        INSERT INTO user_ui_preferences (tenant_id, user_id, key, value)
        VALUES ($1, $2, $3, $4)
        ON CONFLICT (tenant_id, user_id, key)
        DO UPDATE SET value = EXCLUDED.value, updated_at = CURRENT_TIMESTAMP
        "#,
    )
    .bind(tenant_id)
    .bind(user_id)
    .bind(key)
    .bind(value)
    .execute(&state.connect_pool)
    .await?;
    Ok(())
}

pub(super) async fn require_biwork_user_settings_update(
    state: &AppState,
    ctx: &PlatformRequestContext,
) -> Result<(), AppError> {
    require_ferriskey_allow(
        state,
        ctx,
        ctx.tenant_id,
        "update",
        "user_settings",
        ctx.platform_user_id.to_string(),
        Some(AuthzContext {
            risk_level: Some("low".to_string()),
            ..Default::default()
        }),
    )
    .await
    .map(|_| ())
}

fn parse_keys(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn client_settings_update_requires_authz_before_write() {
        let source = include_str!("biwork_settings_service.rs");
        let function_start = source
            .find("pub async fn biwork_update_client_settings")
            .expect("settings update handler exists");
        let function_source = &source[function_start..];
        let authz = function_source
            .find("require_biwork_user_settings_update")
            .expect("settings update requires authz");
        let tx_begin = function_source
            .find("state.connect_pool.begin")
            .expect("settings update starts transaction");

        assert!(authz < tx_begin);
    }
}
