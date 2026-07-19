use axum::{Extension, Json, extract::State};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    features::{
        agent_platform::{
            audit::{self, NewAuditLog},
            event_store,
            ferriskey_oidc::PlatformRequestContext,
            models::RunEventInput,
        },
        core::errors::AppError,
    },
    startup::AppState,
};

use super::{
    biwork_channel_connector_store::{
        disable_extension_connectors, disable_extension_connectors_tx,
    },
    biwork_compat_service::{epoch_ms, ok, required_string, trimmed_string, value_string},
    biwork_event_support::latest_user_conversation_id,
};

#[derive(Debug, Deserialize)]
pub struct ExtensionSyncPayload {
    extensions: Vec<ExtensionSyncPackage>,
}

#[derive(Debug, Deserialize)]
struct ExtensionSyncPackage {
    name: String,
    source: Option<String>,
    version: Option<String>,
    integrity: Option<String>,
    manifest: Value,
    #[serde(alias = "riskLevel")]
    risk_level: Option<String>,
    enabled: Option<bool>,
    installed: Option<bool>,
    #[serde(alias = "installStatus")]
    install_status: Option<String>,
    #[serde(alias = "lastError")]
    last_error: Option<String>,
    contributions: Option<Vec<ExtensionSyncContribution>>,
}

#[derive(Debug, Deserialize)]
struct ExtensionSyncContribution {
    #[serde(
        rename = "type",
        alias = "contribution_type",
        alias = "contributionType"
    )]
    contribution_type: String,
    key: String,
    manifest: Value,
    enabled: Option<bool>,
}

pub async fn biwork_sync_extensions(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<ExtensionSyncPayload>,
) -> Result<Json<Value>, AppError> {
    let mut synced_extensions = 0_i64;
    let mut synced_contributions = 0_i64;
    let mut disabled_channel_connectors = 0_u64;
    let mut tx = state.connect_pool.begin().await?;

    for extension in payload.extensions {
        let name = extension.name.trim().to_string();
        if name.is_empty() {
            return Err(AppError::InvalidInput(
                "extension name is required".to_string(),
            ));
        }
        if !extension.manifest.is_object() {
            return Err(AppError::InvalidInput(
                "extension manifest must be an object".to_string(),
            ));
        }

        let source = normalize_extension_source(extension.source.as_deref());
        let version = trimmed_optional_owned(extension.version.as_deref());
        let integrity = trimmed_optional_owned(extension.integrity.as_deref());
        let risk_level = normalize_extension_risk_level(extension.risk_level.as_deref());
        let device_state_only = extension_sync_is_device_state_only(&extension.manifest);
        let package_enabled = extension.enabled.unwrap_or(true);
        let install_status =
            extension_sync_install_status(extension.installed, extension.install_status.as_deref());
        let package_installed = extension
            .installed
            .unwrap_or_else(|| extension_install_status_is_installed(&install_status));
        let device_enabled = package_installed && package_enabled;
        let last_error = trimmed_optional_owned(extension.last_error.as_deref());

        let package = sqlx::query(
            r#"
            INSERT INTO extension_packages (
                tenant_id, extension_name, source, version, integrity, manifest, risk_level, status
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, 'discovered')
            ON CONFLICT (tenant_id, extension_name)
            DO UPDATE SET source = CASE
                              WHEN $8 THEN extension_packages.source
                              ELSE EXCLUDED.source
                          END,
                          version = CASE
                              WHEN $8 THEN extension_packages.version
                              ELSE COALESCE(EXCLUDED.version, extension_packages.version)
                          END,
                          integrity = CASE
                              WHEN $8 THEN extension_packages.integrity
                              ELSE COALESCE(EXCLUDED.integrity, extension_packages.integrity)
                          END,
                          manifest = CASE
                              WHEN $8 THEN extension_packages.manifest
                              WHEN extension_packages.source = 'hub' AND EXCLUDED.source = 'hub'
                                  THEN extension_packages.manifest || EXCLUDED.manifest
                              ELSE EXCLUDED.manifest
                          END,
                          risk_level = CASE
                              WHEN $8 THEN extension_packages.risk_level
                              ELSE EXCLUDED.risk_level
                          END,
                          updated_at = CURRENT_TIMESTAMP
            RETURNING id
            "#,
        )
        .bind(ctx.tenant_id)
        .bind(&name)
        .bind(&source)
        .bind(version.as_deref())
        .bind(integrity.as_deref())
        .bind(&extension.manifest)
        .bind(&risk_level)
        .bind(device_state_only)
        .fetch_one(&mut *tx)
        .await?;
        let package_id: Uuid = package.try_get("id")?;

        sqlx::query(
            r#"
            INSERT INTO device_extension_states (
                tenant_id, device_id, extension_package_id, installed, enabled, install_status, last_error
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            ON CONFLICT (tenant_id, device_id, extension_package_id)
            DO UPDATE SET installed = EXCLUDED.installed,
                          enabled = EXCLUDED.enabled,
                          install_status = EXCLUDED.install_status,
                          last_error = EXCLUDED.last_error,
                          updated_at = CURRENT_TIMESTAMP
            "#,
        )
        .bind(ctx.tenant_id)
        .bind(ctx.device_id)
        .bind(package_id)
        .bind(package_installed)
        .bind(device_enabled)
        .bind(&install_status)
        .bind(last_error.as_deref())
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            r#"
            UPDATE extension_contributions
            SET enabled = FALSE, updated_at = CURRENT_TIMESTAMP
            WHERE tenant_id = $1 AND extension_package_id = $2
            "#,
        )
        .bind(ctx.tenant_id)
        .bind(package_id)
        .execute(&mut *tx)
        .await?;

        for contribution in extension.contributions.unwrap_or_default() {
            let contribution_type = normalize_extension_contribution_type(
                &contribution.contribution_type,
            )
            .ok_or_else(|| {
                AppError::InvalidInput(format!(
                    "unsupported extension contribution type: {}",
                    contribution.contribution_type
                ))
            })?;
            let key = contribution.key.trim().to_string();
            if key.is_empty() {
                return Err(AppError::InvalidInput(
                    "extension contribution key is required".to_string(),
                ));
            }
            if !contribution.manifest.is_object() {
                return Err(AppError::InvalidInput(
                    "extension contribution manifest must be an object".to_string(),
                ));
            }
            let contribution_enabled =
                package_installed && package_enabled && contribution.enabled.unwrap_or(true);
            sqlx::query(
                r#"
                INSERT INTO extension_contributions (
                    tenant_id, extension_package_id, contribution_type, contribution_key, manifest, enabled
                )
                VALUES ($1, $2, $3, $4, $5, $6)
                ON CONFLICT (tenant_id, extension_package_id, contribution_type, contribution_key)
                DO UPDATE SET manifest = EXCLUDED.manifest,
                              enabled = EXCLUDED.enabled,
                              updated_at = CURRENT_TIMESTAMP
                "#,
            )
            .bind(ctx.tenant_id)
            .bind(package_id)
            .bind(contribution_type)
            .bind(&key)
            .bind(&contribution.manifest)
            .bind(contribution_enabled)
            .execute(&mut *tx)
            .await?;
            synced_contributions += 1;
        }

        if !package_enabled {
            disabled_channel_connectors = disabled_channel_connectors.saturating_add(
                disable_extension_connectors_tx(&mut tx, ctx.tenant_id, package_id).await?,
            );
        }

        synced_extensions += 1;
    }

    tx.commit()
        .await
        .map_err(|_| AppError::DatabaseTransaction)?;
    let synced_extensions_text = synced_extensions.to_string();
    let synced_contributions_text = synced_contributions.to_string();
    let disabled_channel_connectors_text = disabled_channel_connectors.to_string();
    write_extension_audit(
        &state,
        &ctx,
        ExtensionAudit {
            resource_id: format!("sync:{}", ctx.device_id),
            action: "sync_manifest",
            decision: "allow",
            reason_code: Some("extension.sync"),
            output_summary: Some(extension_audit_summary(&[
                ("extensions", synced_extensions_text.as_str()),
                ("contributions", synced_contributions_text.as_str()),
                (
                    "disabled_channel_connectors",
                    disabled_channel_connectors_text.as_str(),
                ),
            ])),
            risk_level: "medium",
        },
    )
    .await?;

    Ok(ok(json!({
        "synced": synced_extensions,
        "contributions": synced_contributions,
    })))
}

pub async fn biwork_list_extensions(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
) -> Result<Json<Value>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT p.extension_name, p.source, p.version, p.manifest, p.status,
               COALESCE(s.enabled, p.status = 'approved') AS enabled,
               COALESCE(s.installed, FALSE) AS installed,
               COALESCE(s.install_status, 'not_installed') AS install_status,
               s.last_error
        FROM extension_packages p
        LEFT JOIN device_extension_states s
          ON s.extension_package_id = p.id
         AND s.tenant_id = p.tenant_id
         AND s.device_id = $2
        WHERE p.tenant_id = $1 AND p.status <> 'blocked'
        ORDER BY p.extension_name
        LIMIT 500
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(ctx.device_id)
    .fetch_all(&state.connect_pool)
    .await?;
    let extensions = rows
        .iter()
        .map(|row| {
            let manifest: Value = row.try_get("manifest")?;
            Ok(extension_list_item_json(ExtensionListItem {
                name: row.try_get::<String, _>("extension_name")?,
                source: row.try_get::<String, _>("source")?,
                version: row.try_get::<Option<String>, _>("version")?,
                manifest,
                enabled: row.try_get::<bool, _>("enabled")?,
                installed: row.try_get::<bool, _>("installed")?,
                install_status: row.try_get::<String, _>("install_status")?,
                install_error: row.try_get::<Option<String>, _>("last_error")?,
            }))
        })
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(ok(Value::Array(extensions)))
}

struct ExtensionListItem {
    name: String,
    source: String,
    version: Option<String>,
    manifest: Value,
    enabled: bool,
    installed: bool,
    install_status: String,
    install_error: Option<String>,
}

fn extension_list_item_json(item: ExtensionListItem) -> Value {
    let ExtensionListItem {
        name,
        source,
        version,
        manifest,
        enabled,
        installed,
        install_status,
        install_error,
    } = item;
    json!({
        "name": name,
        "display_name": value_string(&manifest, "display_name")
            .or_else(|| value_string(&manifest, "displayName"))
            .unwrap_or_else(|| name.clone()),
        "version": version.unwrap_or_else(|| "0.0.0".to_string()),
        "description": value_string(&manifest, "description").unwrap_or_default(),
        "source": source,
        "enabled": enabled,
        "installed": installed,
        "install_status": install_status,
        "installError": install_error,
    })
}

pub async fn biwork_list_extension_themes(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
) -> Result<Json<Value>, AppError> {
    list_extension_contributions(&state, ctx.tenant_id, ctx.device_id, "theme").await
}

pub async fn biwork_list_extension_assistants(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
) -> Result<Json<Value>, AppError> {
    list_extension_contributions(&state, ctx.tenant_id, ctx.device_id, "assistant").await
}

pub async fn biwork_list_extension_agents(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
) -> Result<Json<Value>, AppError> {
    list_extension_contributions(&state, ctx.tenant_id, ctx.device_id, "agent").await
}

pub async fn biwork_list_extension_acp_adapters(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
) -> Result<Json<Value>, AppError> {
    list_extension_contributions(&state, ctx.tenant_id, ctx.device_id, "acp_adapter").await
}

pub async fn biwork_list_extension_channel_plugins(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
) -> Result<Json<Value>, AppError> {
    list_extension_contributions(&state, ctx.tenant_id, ctx.device_id, "channel_plugin").await
}

pub async fn biwork_list_extension_mcp_servers(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
) -> Result<Json<Value>, AppError> {
    list_extension_contributions(&state, ctx.tenant_id, ctx.device_id, "mcp_server").await
}

pub async fn biwork_list_extension_skills(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
) -> Result<Json<Value>, AppError> {
    list_extension_contributions(&state, ctx.tenant_id, ctx.device_id, "skill").await
}

pub async fn biwork_list_extension_settings_tabs(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
) -> Result<Json<Value>, AppError> {
    let rows =
        extension_contribution_rows(&state, ctx.tenant_id, ctx.device_id, "settings_tab").await?;
    let tabs = rows
        .iter()
        .enumerate()
        .map(|(idx, row)| {
            let extension_name: String = row.try_get("extension_name")?;
            let key: String = row.try_get("contribution_key")?;
            let manifest: Value = row.try_get("manifest")?;
            let url = manifest
                .get("url")
                .and_then(Value::as_str)
                .filter(|value| value.starts_with("/api/extensions/static/"))
                .map(str::to_string)
                .unwrap_or_else(|| format!("/api/extensions/static/{extension_name}/{key}"));
            Ok(json!({
                "id": key,
                "label": value_string(&manifest, "label").unwrap_or_else(|| extension_name.clone()),
                "icon": manifest.get("icon").cloned().unwrap_or(Value::Null),
                "url": url,
                "order": manifest.get("order").and_then(Value::as_i64).unwrap_or(idx as i64),
                "extensionName": extension_name,
            }))
        })
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(ok(Value::Array(tabs)))
}

pub async fn biwork_list_extension_webui(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
) -> Result<Json<Value>, AppError> {
    list_extension_contributions(&state, ctx.tenant_id, ctx.device_id, "webui").await
}

pub async fn biwork_extension_agent_activity(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
) -> Result<Json<Value>, AppError> {
    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM conversations WHERE tenant_id = $1 AND deleted_at IS NULL",
    )
    .bind(ctx.tenant_id)
    .fetch_one(&state.connect_pool)
    .await?;
    let running: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM runs WHERE tenant_id = $1 AND status IN ('queued', 'running')",
    )
    .bind(ctx.tenant_id)
    .fetch_one(&state.connect_pool)
    .await?;
    Ok(ok(json!({
        "generatedAt": epoch_ms(OffsetDateTime::now_utc()),
        "totalConversations": total,
        "runningConversations": running,
        "agents": [],
    })))
}

pub async fn biwork_extension_i18n() -> Result<Json<Value>, AppError> {
    Ok(ok(json!({})))
}

pub async fn biwork_enable_extension(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    set_extension_enabled(&state, &ctx, &payload, true).await
}

pub async fn biwork_disable_extension(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    set_extension_enabled(&state, &ctx, &payload, false).await
}

pub async fn biwork_extension_permissions(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let name = required_string(&payload, "name")?;
    let row = load_extension_package(&state, ctx.tenant_id, &name).await?;
    let manifest: Value = row.try_get("manifest")?;
    let permissions = manifest
        .get("permissions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(Value::is_object)
        .collect::<Vec<_>>();
    Ok(ok(Value::Array(permissions)))
}

pub async fn biwork_extension_risk_level(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let name = required_string(&payload, "name")?;
    let row = load_extension_package(&state, ctx.tenant_id, &name).await?;
    Ok(ok(json!(row.try_get::<String, _>("risk_level")?)))
}

pub async fn biwork_list_hub_extensions(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
) -> Result<Json<Value>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT p.extension_name, p.version, p.manifest, p.source, p.integrity,
               COALESCE(s.install_status, 'not_installed') AS install_status,
               s.last_error
        FROM extension_packages p
        LEFT JOIN device_extension_states s
          ON s.extension_package_id = p.id
         AND s.tenant_id = p.tenant_id
         AND s.device_id = $2
        WHERE p.tenant_id = $1 AND p.status <> 'blocked'
        ORDER BY p.extension_name
        LIMIT 500
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(ctx.device_id)
    .fetch_all(&state.connect_pool)
    .await?;
    let items = rows
        .iter()
        .map(hub_extension_from_row)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ok(Value::Array(items)))
}

pub async fn biwork_hub_local_runtime_required() -> Result<Json<Value>, AppError> {
    Err(AppError::InvalidInput(
        "local hub extension runtime is not attached".to_string(),
    ))
}

fn normalize_extension_source(value: Option<&str>) -> String {
    let Some(source) = trimmed_optional_owned(value).map(|value| value.to_ascii_lowercase()) else {
        return "local".to_string();
    };
    match source.as_str() {
        "local" | "bundled" | "hub" | "marketplace" => source,
        _ => "local".to_string(),
    }
}

fn trimmed_optional_owned(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn extension_sync_is_device_state_only(manifest: &Value) -> bool {
    manifest
        .get("local_status")
        .and_then(Value::as_str)
        .is_some()
}

fn normalize_extension_risk_level(value: Option<&str>) -> String {
    let Some(risk_level) = trimmed_optional_owned(value).map(|value| value.to_ascii_lowercase())
    else {
        return "moderate".to_string();
    };
    match risk_level.as_str() {
        "safe" | "moderate" | "high" | "critical" => risk_level,
        _ => "moderate".to_string(),
    }
}

fn normalize_extension_install_status(value: Option<&str>) -> String {
    let Some(status) = trimmed_optional_owned(value).map(|value| value.to_ascii_lowercase()) else {
        return "installed".to_string();
    };
    match status.as_str() {
        "not_installed" | "installing" | "installed" | "install_failed" | "update_available"
        | "uninstalling" => status,
        _ => "installed".to_string(),
    }
}

fn extension_sync_install_status(installed: Option<bool>, value: Option<&str>) -> String {
    match value {
        Some(status) => normalize_extension_install_status(Some(status)),
        None if installed == Some(false) => "not_installed".to_string(),
        None => normalize_extension_install_status(None),
    }
}

fn extension_install_status_is_installed(status: &str) -> bool {
    matches!(status, "installed" | "update_available")
}

fn normalize_extension_contribution_type(value: &str) -> Option<&'static str> {
    match value.trim() {
        "theme" | "themes" => Some("theme"),
        "assistant" | "assistants" => Some("assistant"),
        "agent" | "agents" => Some("agent"),
        "acp_adapter" | "acp_adapters" | "acpAdapter" | "acpAdapters" => Some("acp_adapter"),
        "mcp_server" | "mcp_servers" | "mcpServer" | "mcpServers" => Some("mcp_server"),
        "skill" | "skills" => Some("skill"),
        "settings_tab" | "settings_tabs" | "settingsTab" | "settingsTabs" => Some("settings_tab"),
        "webui" => Some("webui"),
        "channel_plugin" | "channel_plugins" | "channelPlugin" | "channelPlugins" => {
            Some("channel_plugin")
        }
        _ => None,
    }
}

async fn emit_extension_state_changed_event(
    state: &AppState,
    ctx: &PlatformRequestContext,
    payload: Value,
) -> Result<(), AppError> {
    let Some(conversation_id) = latest_user_conversation_id(state, ctx).await? else {
        return Ok(());
    };
    let name = payload
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let mut tx = state.connect_pool.begin().await?;
    let event = event_store::insert_event_tx(
        &mut tx,
        ctx.tenant_id,
        conversation_id,
        None,
        RunEventInput {
            event_id: Some(format!(
                "extensions.state-changed.{name}.{}",
                Uuid::new_v4()
            )),
            event_type: "extensions.state-changed".to_string(),
            payload: Some(payload),
            trace_id: Some(ctx.trace_id.clone()),
        },
    )
    .await?;
    tx.commit()
        .await
        .map_err(|_| AppError::DatabaseTransaction)?;
    event_store::publish_single_event(state, &event).await;
    Ok(())
}

struct ExtensionAudit<'a> {
    resource_id: String,
    action: &'a str,
    decision: &'a str,
    reason_code: Option<&'a str>,
    output_summary: Option<String>,
    risk_level: &'a str,
}

async fn write_extension_audit(
    state: &AppState,
    ctx: &PlatformRequestContext,
    entry: ExtensionAudit<'_>,
) -> Result<(), AppError> {
    let ExtensionAudit {
        resource_id,
        action,
        decision,
        reason_code,
        output_summary,
        risk_level,
    } = entry;
    let mut tx = state.connect_pool.begin().await?;
    audit::insert_audit_log_tx(
        &mut tx,
        NewAuditLog {
            tenant_id: ctx.tenant_id,
            actor_user_id: Some(ctx.platform_user_id),
            actor_device_id: Some(ctx.device_id),
            session_id: Some(ctx.session_id),
            resource_type: "extension",
            resource_id: &resource_id,
            action,
            decision,
            policy_version: "biwork-extension-v1",
            reason_code,
            run_id: None,
            conversation_id: None,
            workflow_run_id: None,
            tool_call_id: None,
            approval_id: None,
            args_hash: None,
            input_summary: Some(&resource_id),
            output_summary: output_summary.as_deref(),
            risk_level: Some(risk_level),
            ip: None,
            user_agent: None,
            trace_id: Some(ctx.trace_id.as_str()),
        },
    )
    .await?;
    tx.commit().await.map_err(|_| AppError::DatabaseTransaction)
}

fn extension_audit_summary(fields: &[(&str, &str)]) -> String {
    fields
        .iter()
        .filter(|(_, value)| !value.trim().is_empty())
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("; ")
}

fn extension_audit_risk_level(risk_level: &str) -> &'static str {
    match risk_level.trim().to_ascii_lowercase().as_str() {
        "safe" => "low",
        "high" => "high",
        "critical" => "critical",
        _ => "medium",
    }
}

fn extension_state_changed_payload(name: &str, enabled: bool, reason: &str) -> Value {
    json!({
        "name": name,
        "enabled": enabled,
        "reason": reason,
    })
}

pub(super) async fn extension_contribution_rows(
    state: &AppState,
    tenant_id: Uuid,
    device_id: Uuid,
    contribution_type: &str,
) -> Result<Vec<sqlx::postgres::PgRow>, AppError> {
    Ok(sqlx::query(
        r#"
        SELECT c.contribution_key, c.manifest, p.extension_name
        FROM extension_contributions c
        JOIN extension_packages p
          ON p.id = c.extension_package_id AND p.tenant_id = c.tenant_id
        JOIN device_extension_states s
          ON s.extension_package_id = c.extension_package_id
         AND s.tenant_id = c.tenant_id
         AND s.device_id = $3
        WHERE c.tenant_id = $1
          AND c.contribution_type = $2
          AND c.enabled = TRUE
          AND s.installed = TRUE
          AND s.enabled = TRUE
          AND s.install_status = 'installed'
          AND (
              ($2 = 'webui' AND p.status = 'approved')
              OR ($2 <> 'webui' AND p.status IN ('discovered', 'approved'))
          )
        ORDER BY p.extension_name, c.contribution_key
        LIMIT 500
        "#,
    )
    .bind(tenant_id)
    .bind(contribution_type)
    .bind(device_id)
    .fetch_all(&state.connect_pool)
    .await?)
}

pub(super) async fn resolve_channel_plugin_package(
    state: &AppState,
    tenant_id: Uuid,
    device_id: Uuid,
    plugin_id: &str,
) -> Result<Option<Uuid>, AppError> {
    sqlx::query_scalar(
        r#"
        SELECT c.extension_package_id
        FROM extension_contributions c
        JOIN extension_packages p
          ON p.id = c.extension_package_id AND p.tenant_id = c.tenant_id
        JOIN device_extension_states s
          ON s.extension_package_id = c.extension_package_id
         AND s.tenant_id = c.tenant_id
         AND s.device_id = $3
        WHERE c.tenant_id = $1
          AND c.contribution_type = 'channel_plugin'
          AND c.contribution_key = $2
          AND c.enabled = TRUE
          AND s.installed = TRUE
          AND s.enabled = TRUE
          AND s.install_status = 'installed'
          AND p.status IN ('discovered', 'approved')
        ORDER BY p.extension_name
        LIMIT 1
        "#,
    )
    .bind(tenant_id)
    .bind(plugin_id)
    .bind(device_id)
    .fetch_optional(&state.connect_pool)
    .await
    .map_err(Into::into)
}

async fn list_extension_contributions(
    state: &AppState,
    tenant_id: Uuid,
    device_id: Uuid,
    contribution_type: &str,
) -> Result<Json<Value>, AppError> {
    let rows = extension_contribution_rows(state, tenant_id, device_id, contribution_type).await?;
    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        let mut manifest = row.try_get::<Value, _>("manifest")?;
        if let Some(object) = manifest.as_object_mut() {
            object
                .entry("extensionName".to_string())
                .or_insert_with(|| {
                    json!(
                        row.try_get::<String, _>("extension_name")
                            .unwrap_or_default()
                    )
                });
            object.entry("key".to_string()).or_insert_with(|| {
                json!(
                    row.try_get::<String, _>("contribution_key")
                        .unwrap_or_default()
                )
            });
        }
        items.push(manifest);
    }
    Ok(ok(Value::Array(items)))
}

async fn load_extension_package(
    state: &AppState,
    tenant_id: Uuid,
    name: &str,
) -> Result<sqlx::postgres::PgRow, AppError> {
    sqlx::query(
        r#"
        SELECT *
        FROM extension_packages
        WHERE tenant_id = $1 AND extension_name = $2 AND status <> 'blocked'
        "#,
    )
    .bind(tenant_id)
    .bind(name)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("extension not found".to_string()))
}

async fn set_extension_enabled(
    state: &AppState,
    ctx: &PlatformRequestContext,
    payload: &Value,
    enabled: bool,
) -> Result<Json<Value>, AppError> {
    let name = required_string(payload, "name")?;
    let reason = trimmed_string(payload, "reason").unwrap_or_else(|| "user_toggle".to_string());
    let package = load_extension_package(state, ctx.tenant_id, &name).await?;
    let package_id: Uuid = package.try_get("id")?;
    let risk_level: String = package.try_get("risk_level")?;
    sqlx::query(
        r#"
        INSERT INTO device_extension_states (
            tenant_id, device_id, extension_package_id, installed, enabled, install_status, last_error
        )
        VALUES ($1, $2, $3, TRUE, $4, 'installed', NULL)
        ON CONFLICT (tenant_id, device_id, extension_package_id)
        DO UPDATE SET enabled = EXCLUDED.enabled,
                      install_status = 'installed',
                      last_error = NULL,
                      updated_at = CURRENT_TIMESTAMP
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(ctx.device_id)
    .bind(package_id)
    .bind(enabled)
    .execute(&state.connect_pool)
    .await?;
    let disabled_channel_connectors = if enabled {
        0
    } else {
        disable_extension_connectors(state, ctx.tenant_id, package_id).await?
    };
    let enabled_text = enabled.to_string();
    let package_id_text = package_id.to_string();
    let disabled_channel_connectors_text = disabled_channel_connectors.to_string();
    write_extension_audit(
        state,
        ctx,
        ExtensionAudit {
            resource_id: format!("package:{package_id}"),
            action: if enabled {
                "enable_extension"
            } else {
                "disable_extension"
            },
            decision: "allow",
            reason_code: Some(if enabled {
                "extension.enable"
            } else {
                "extension.disable"
            }),
            output_summary: Some(extension_audit_summary(&[
                ("name", name.as_str()),
                ("package_id", package_id_text.as_str()),
                ("enabled", enabled_text.as_str()),
                ("reason", reason.as_str()),
                (
                    "disabled_channel_connectors",
                    disabled_channel_connectors_text.as_str(),
                ),
            ])),
            risk_level: extension_audit_risk_level(&risk_level),
        },
    )
    .await?;
    emit_extension_state_changed_event(
        state,
        ctx,
        extension_state_changed_payload(&name, enabled, &reason),
    )
    .await?;
    Ok(ok(Value::Null))
}

fn hub_extension_from_row(row: &sqlx::postgres::PgRow) -> Result<Value, AppError> {
    let name: String = row.try_get("extension_name")?;
    let manifest: Value = row.try_get("manifest")?;
    let status: String = row.try_get("install_status")?;
    Ok(json!({
        "name": name,
        "display_name": value_string(&manifest, "display_name")
            .or_else(|| value_string(&manifest, "displayName"))
            .unwrap_or_else(|| row.try_get::<String, _>("extension_name").unwrap_or_default()),
        "version": row.try_get::<Option<String>, _>("version")?.unwrap_or_else(|| "0.0.0".to_string()),
        "description": value_string(&manifest, "description").unwrap_or_default(),
        "author": value_string(&manifest, "author").unwrap_or_else(|| "enterprise".to_string()),
        "icon": manifest.get("icon").cloned().unwrap_or(Value::Null),
        "dist": manifest.get("dist").cloned().unwrap_or_else(|| json!({
            "tarball": "",
            "integrity": row.try_get::<Option<String>, _>("integrity").unwrap_or_default().unwrap_or_default(),
            "unpackedSize": 0,
        })),
        "engines": manifest.get("engines").cloned().unwrap_or_else(|| json!({ "biwork": "*" })),
        "hubs": manifest.get("hubs").cloned().unwrap_or_else(|| json!([])),
        "contributes": manifest.get("contributes").cloned().unwrap_or_else(|| json!({})),
        "tags": manifest.get("tags").cloned().unwrap_or_else(|| json!([])),
        "bundled": row.try_get::<String, _>("source")? == "bundled",
        "status": status,
        "installError": row.try_get::<Option<String>, _>("last_error")?,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_sync_normalizes_sources_risks_and_contribution_types() {
        assert_eq!(normalize_extension_source(None), "local");
        assert_eq!(normalize_extension_source(Some(" HUB ")), "hub");
        assert_eq!(normalize_extension_source(Some("invalid")), "local");

        assert_eq!(normalize_extension_risk_level(None), "moderate");
        assert_eq!(normalize_extension_risk_level(Some(" HIGH ")), "high");
        assert_eq!(normalize_extension_risk_level(Some("invalid")), "moderate");

        assert_eq!(
            normalize_extension_install_status(Some(" install_failed ")),
            "install_failed"
        );
        assert_eq!(
            normalize_extension_install_status(Some("invalid")),
            "installed"
        );
        assert_eq!(
            extension_sync_install_status(Some(false), None),
            "not_installed"
        );
        assert_eq!(extension_sync_install_status(Some(true), None), "installed");
        assert!(!extension_install_status_is_installed("install_failed"));
        assert!(!extension_install_status_is_installed("not_installed"));
        assert!(extension_install_status_is_installed("installed"));
        assert!(extension_install_status_is_installed("update_available"));
        assert!(extension_sync_is_device_state_only(&json!({
            "name": "hub-extension",
            "local_status": "not_installed",
        })));
        assert!(!extension_sync_is_device_state_only(&json!({
            "name": "hub-extension",
            "dist": { "tarball": "https://example.invalid/ext.tgz" },
        })));

        assert_eq!(
            normalize_extension_contribution_type("channelPlugins"),
            Some("channel_plugin")
        );
        assert_eq!(
            normalize_extension_contribution_type("settings_tab"),
            Some("settings_tab")
        );
        assert_eq!(
            normalize_extension_contribution_type("acpAdapters"),
            Some("acp_adapter")
        );
        assert_eq!(normalize_extension_contribution_type("unknown"), None);
    }

    #[test]
    fn extension_audit_summary_and_risk_use_stable_platform_fields() {
        let summary = extension_audit_summary(&[
            ("name", "theme-pack"),
            ("package_id", "pkg-1"),
            ("secret", ""),
        ]);

        assert_eq!(summary, "name=theme-pack; package_id=pkg-1");
        assert_eq!(extension_audit_risk_level(" safe "), "low");
        assert_eq!(extension_audit_risk_level("moderate"), "medium");
        assert_eq!(extension_audit_risk_level("HIGH"), "high");
        assert_eq!(extension_audit_risk_level("critical"), "critical");
        assert_eq!(extension_audit_risk_level("unknown"), "medium");
    }

    #[test]
    fn extension_state_changed_payload_matches_biwork_contract() {
        let payload = extension_state_changed_payload("theme-pack", false, "policy_sync");

        assert_eq!(payload["name"], "theme-pack");
        assert_eq!(payload["enabled"], false);
        assert_eq!(payload["reason"], "policy_sync");
        assert!(payload.get("payload").is_none());
    }

    #[test]
    fn extension_list_item_exposes_device_install_state() {
        let item = extension_list_item_json(ExtensionListItem {
            name: "theme-pack".to_string(),
            source: "hub".to_string(),
            version: Some("1.2.3".to_string()),
            manifest: json!({
                "displayName": "Theme Pack",
                "description": "A hub theme pack"
            }),
            enabled: false,
            installed: false,
            install_status: "install_failed".to_string(),
            install_error: Some("installer unavailable".to_string()),
        });

        assert_eq!(item["name"], "theme-pack");
        assert_eq!(item["display_name"], "Theme Pack");
        assert_eq!(item["source"], "hub");
        assert_eq!(item["enabled"], false);
        assert_eq!(item["installed"], false);
        assert_eq!(item["install_status"], "install_failed");
        assert_eq!(item["installError"], "installer unavailable");
    }
}
