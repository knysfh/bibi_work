use axum::{
    Extension, Json,
    extract::{Path, Query, State},
};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    features::{
        agent_platform::{
            ferriskey_oidc::PlatformRequestContext,
            models::{
                ActorRef, ApprovalDecisionRequest, AuthzCheckRequest, AuthzContext, ResourceRef,
            },
        },
        core::errors::AppError,
    },
    startup::AppState,
};

use super::{
    approval_service::decide_approval,
    biwork_compat_service::{epoch_ms, ok, value_string},
    biwork_conversation_support::ensure_conversation_exists,
    support::write_authz_audit,
};

#[derive(Debug, Deserialize)]
pub struct ApprovalCheckQuery {
    action: String,
    command_type: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ConfirmationDecisionPayload {
    msg_id: Option<String>,
    data: Option<Value>,
    always_allow: Option<bool>,
}

pub async fn biwork_list_conversation_confirmations(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(conversation_id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    ensure_conversation_exists(&state, ctx.tenant_id, conversation_id).await?;
    let rows = sqlx::query(
        r#"
        SELECT a.id,
               a.tool_call_id,
               a.request_payload,
               a.created_at,
               t.tool_name,
               t.resource_type,
               t.risk_level,
               t.input_summary
        FROM approvals a
        LEFT JOIN tool_calls t ON t.id = a.tool_call_id
        WHERE a.tenant_id = $1
          AND a.conversation_id = $2
          AND a.status = 'pending'
        ORDER BY a.created_at ASC
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(conversation_id)
    .fetch_all(&state.connect_pool)
    .await?;

    let confirmations = rows
        .iter()
        .map(biwork_confirmation_from_row)
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(ok(json!(confirmations)))
}

pub async fn biwork_confirm_conversation_confirmation(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path((conversation_id, call_id)): Path<(Uuid, String)>,
    Json(payload): Json<ConfirmationDecisionPayload>,
) -> Result<Json<Value>, AppError> {
    ensure_conversation_exists(&state, ctx.tenant_id, conversation_id).await?;
    let call_uuid = Uuid::parse_str(&call_id).ok();
    let row = sqlx::query(
        r#"
        SELECT id, tenant_id
        FROM approvals
        WHERE tenant_id = $1
          AND conversation_id = $2
          AND status = 'pending'
          AND (
              ($3::uuid IS NOT NULL AND (id = $3 OR tool_call_id = $3))
              OR id::text = $4
              OR tool_call_id::text = $4
          )
        ORDER BY created_at DESC
        LIMIT 1
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(conversation_id)
    .bind(call_uuid)
    .bind(call_id.trim())
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("pending confirmation not found".to_string()))?;

    let approval_id: Uuid = row.try_get("id")?;
    let tenant_id: Uuid = row.try_get("tenant_id")?;
    let selected = confirmation_selected_value(&payload);
    let decision = confirmation_decision(&selected)?;
    let always_allow = confirmation_always_allow(&selected, payload.always_allow);
    let Json(approval) = decide_approval(
        State(state),
        Extension(ctx),
        Path(approval_id),
        Json(ApprovalDecisionRequest {
            tenant_id,
            decision,
            reason: Some("BiWork confirmation response".to_string()),
            payload: Some(json!({
                "source": "biwork",
                "call_id": call_id,
                "msg_id": payload.msg_id,
                "selected": selected,
                "always_allow": always_allow,
            })),
        }),
    )
    .await?;

    Ok(ok(json!({
        "approval_id": approval.id.to_string(),
        "status": approval.status,
    })))
}

pub async fn biwork_check_conversation_approval(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(conversation_id): Path<Uuid>,
    Query(query): Query<ApprovalCheckQuery>,
) -> Result<Json<Value>, AppError> {
    ensure_conversation_exists(&state, ctx.tenant_id, conversation_id).await?;
    let pending_count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)::BIGINT
        FROM approvals
        WHERE tenant_id = $1
          AND conversation_id = $2
          AND status = 'pending'
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(conversation_id)
    .fetch_one(&state.connect_pool)
    .await?;
    if pending_count > 0 {
        return Ok(ok(json!({
            "approved": false,
            "decision": "review",
            "reason_code": "pending_confirmation",
        })));
    }

    let checks_tool = query.command_type.is_some()
        || query.action.contains("exec")
        || query.action.contains("tool");
    let resource_type = if checks_tool { "tool" } else { "conversation" };
    let resource_id = query
        .command_type
        .clone()
        .unwrap_or_else(|| conversation_id.to_string());
    let action = if checks_tool {
        "execute".to_string()
    } else {
        query.action.clone()
    };
    let request = AuthzCheckRequest {
        tenant_id: ctx.tenant_id,
        actor: ActorRef {
            user_id: ctx.platform_user_id,
            device_id: Some(ctx.device_id),
            session_id: Some(ctx.session_id),
            roles: ctx.roles.clone(),
        },
        action,
        resource: ResourceRef {
            resource_type: resource_type.to_string(),
            id: resource_id,
            path: None,
        },
        context: Some(AuthzContext {
            conversation_id: Some(conversation_id),
            risk_level: Some("low".to_string()),
            trace_id: Some(ctx.trace_id.clone()),
            ..Default::default()
        }),
    };
    let decision = state.authz_service.check(&request).await;
    write_authz_audit(&state.connect_pool, &request, &decision).await?;

    Ok(ok(json!({
        "approved": decision.is_allow(),
        "decision": decision.decision,
        "reason_code": decision.reason_code,
    })))
}

fn biwork_confirmation_from_row(row: &sqlx::postgres::PgRow) -> Result<Value, AppError> {
    let approval_id: Uuid = row.try_get("id")?;
    let tool_call_id: Option<Uuid> = row.try_get("tool_call_id")?;
    let request_payload: Value = row.try_get("request_payload")?;
    let created_at: OffsetDateTime = row.try_get("created_at")?;
    let tool_name = optional_non_empty(row.try_get::<Option<String>, _>("tool_name")?);
    let resource_type = optional_non_empty(row.try_get::<Option<String>, _>("resource_type")?);
    let risk_level = optional_non_empty(row.try_get::<Option<String>, _>("risk_level")?);
    let input_summary = optional_non_empty(row.try_get::<Option<String>, _>("input_summary")?);
    let command_type = tool_name
        .as_deref()
        .map(biwork_command_type)
        .filter(|value| !value.is_empty());
    let is_browser = tool_name
        .as_deref()
        .is_some_and(|name| name.starts_with("browser_"));
    let action = if is_browser {
        "browser"
    } else if resource_type.as_deref() == Some("mcp") {
        "mcp"
    } else {
        "exec"
    };
    let tool_label = tool_name
        .clone()
        .unwrap_or_else(|| "tool execution".to_string());
    let description = input_summary
        .or_else(|| value_string(&request_payload, "input_summary"))
        .or_else(|| value_string(&request_payload, "summary"))
        .or_else(|| {
            request_payload
                .pointer("/authz/reason_code")
                .and_then(Value::as_str)
                .map(|reason| format!("Policy requires review: {reason}"))
        })
        .or_else(|| {
            risk_level
                .as_deref()
                .map(|risk| format!("Approve {risk} risk tool call: {tool_label}"))
        })
        .unwrap_or_else(|| format!("Approve tool call: {tool_label}"));

    Ok(biwork_confirmation_contract_json(
        approval_id,
        tool_call_id,
        &tool_label,
        action,
        description,
        command_type,
        created_at,
    ))
}

pub(super) fn biwork_confirmation_contract_json(
    approval_id: Uuid,
    tool_call_id: Option<Uuid>,
    tool_label: &str,
    action: &str,
    description: String,
    command_type: Option<String>,
    created_at: OffsetDateTime,
) -> Value {
    let (title, options) = if action == "browser" {
        (
            format!("Continue browser task: {tool_label}"),
            json!([
                { "label": "I have finished, continue", "value": "proceed" },
                { "label": "Cancel", "value": "cancel" }
            ]),
        )
    } else {
        (
            format!("Approve {tool_label}"),
            json!([
                { "label": "Allow once", "value": "proceed_once" },
                { "label": "Allow always", "value": "proceed_always" },
                { "label": "Cancel", "value": "cancel" }
            ]),
        )
    };
    json!({
        "id": approval_id.to_string(),
        "approval_id": approval_id.to_string(),
        "call_id": tool_call_id.unwrap_or(approval_id).to_string(),
        "title": title,
        "action": action,
        "description": description,
        "command_type": command_type,
        "created_at": epoch_ms(created_at),
        "options": options,
    })
}

fn optional_non_empty(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn biwork_command_type(tool_name: &str) -> String {
    tool_name
        .trim()
        .split([':', '/', ' '])
        .next()
        .unwrap_or(tool_name)
        .to_ascii_lowercase()
}

fn confirmation_selected_value(payload: &ConfirmationDecisionPayload) -> String {
    let Some(data) = payload.data.as_ref() else {
        return if payload.always_allow.unwrap_or(false) {
            "proceed_always".to_string()
        } else {
            "proceed_once".to_string()
        };
    };
    if let Some(value) = data.as_str() {
        return value.to_string();
    }
    ["value", "confirm_key", "key", "decision", "type"]
        .into_iter()
        .find_map(|key| data.get(key).and_then(Value::as_str).map(str::to_string))
        .unwrap_or_else(|| {
            if payload.always_allow.unwrap_or(false) {
                "proceed_always".to_string()
            } else {
                "proceed_once".to_string()
            }
        })
}

pub(super) fn confirmation_decision(selected: &str) -> Result<String, AppError> {
    match selected.trim().to_ascii_lowercase().as_str() {
        "approve"
        | "approved"
        | "allow"
        | "allowed"
        | "allow_once"
        | "allow_always"
        | "yes"
        | "proceed"
        | "proceed_once"
        | "proceed_always"
        | "proceed_always_server"
        | "proceed_always_tool" => Ok("approved".to_string()),
        "reject" | "rejected" | "reject_once" | "reject_always" | "deny" | "denied" | "no"
        | "cancel" | "canceled" | "cancelled" => Ok("rejected".to_string()),
        other => Err(AppError::InvalidInput(format!(
            "unsupported confirmation decision: {other}"
        ))),
    }
}

pub(super) fn confirmation_always_allow(selected: &str, explicit: Option<bool>) -> bool {
    if explicit.unwrap_or(false) {
        return true;
    }
    matches!(
        selected.trim().to_ascii_lowercase().as_str(),
        "allow_always" | "proceed_always" | "proceed_always_server" | "proceed_always_tool"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::Duration;

    fn offset_datetime_from_epoch_ms(epoch_ms_value: i64) -> Result<OffsetDateTime, AppError> {
        let seconds = epoch_ms_value.div_euclid(1_000);
        let milliseconds = epoch_ms_value.rem_euclid(1_000);
        OffsetDateTime::from_unix_timestamp(seconds)
            .map(|value| value + Duration::milliseconds(milliseconds))
            .map_err(|_| AppError::InvalidInput("timestamp is out of range".to_string()))
    }

    #[test]
    fn confirmation_contract_contains_biwork_required_fields() {
        let approval_id = Uuid::parse_str("00000000-0000-0000-0000-000000000111").unwrap();
        let tool_call_id = Uuid::parse_str("00000000-0000-0000-0000-000000000222").unwrap();
        let created_at = offset_datetime_from_epoch_ms(42_000).expect("valid timestamp");

        let confirmation = biwork_confirmation_contract_json(
            approval_id,
            Some(tool_call_id),
            "shell.exec",
            "exec",
            "Approve command execution".to_string(),
            Some("shell.exec".to_string()),
            created_at,
        );

        assert_eq!(confirmation["id"], approval_id.to_string());
        assert_eq!(confirmation["approval_id"], approval_id.to_string());
        assert_eq!(confirmation["call_id"], tool_call_id.to_string());
        assert_eq!(confirmation["title"], "Approve shell.exec");
        assert_eq!(confirmation["action"], "exec");
        assert_eq!(confirmation["description"], "Approve command execution");
        assert_eq!(confirmation["command_type"], "shell.exec");
        assert_eq!(confirmation["created_at"], 42_000);
        assert_eq!(confirmation["options"][0]["value"], "proceed_once");
        assert_eq!(confirmation["options"][1]["value"], "proceed_always");
        assert_eq!(confirmation["options"][2]["value"], "cancel");
    }

    #[test]
    fn confirmation_decision_accepts_biwork_and_legacy_aliases() {
        for selected in [
            "proceed_once",
            "proceed_always",
            "proceed_always_server",
            "proceed_always_tool",
            "allow_once",
            "allow_always",
        ] {
            assert_eq!(confirmation_decision(selected).unwrap(), "approved");
        }

        for selected in ["cancel", "deny", "reject_once", "reject_always"] {
            assert_eq!(confirmation_decision(selected).unwrap(), "rejected");
        }

        assert!(!confirmation_always_allow("proceed_once", None));
        assert!(confirmation_always_allow("allow_always", None));
        assert!(confirmation_always_allow("proceed_always_tool", None));
        assert!(confirmation_always_allow("cancel", Some(true)));
    }

    #[test]
    fn browser_confirmation_only_allows_continue_or_cancel() {
        let approval_id = Uuid::parse_str("00000000-0000-0000-0000-000000000333").unwrap();
        let created_at = offset_datetime_from_epoch_ms(43_000).expect("valid timestamp");

        let confirmation = biwork_confirmation_contract_json(
            approval_id,
            None,
            "browser_wait_for_user",
            "browser",
            "Complete login in the visible browser, then continue".to_string(),
            Some("browser_wait_for_user".to_string()),
            created_at,
        );

        assert_eq!(confirmation["action"], "browser");
        assert_eq!(confirmation["options"].as_array().unwrap().len(), 2);
        assert_eq!(confirmation["options"][0]["value"], "proceed");
        assert_eq!(confirmation["options"][1]["value"], "cancel");
    }
}
