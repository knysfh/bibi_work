use serde_json::json;
use uuid::Uuid;

use crate::{
    features::{
        agent_platform::{
            event_store, ferriskey_oidc::PlatformRequestContext, models::RunEventInput,
        },
        core::errors::AppError,
    },
    startup::AppState,
};

pub(super) async fn latest_user_conversation_id(
    state: &AppState,
    ctx: &PlatformRequestContext,
) -> Result<Option<Uuid>, AppError> {
    sqlx::query_scalar(
        r#"
        SELECT id
        FROM conversations
        WHERE tenant_id = $1
          AND created_by_user_id = $2
          AND deleted_at IS NULL
        ORDER BY updated_at DESC, created_at DESC
        LIMIT 1
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(ctx.platform_user_id)
    .fetch_optional(&state.connect_pool)
    .await
    .map_err(Into::into)
}

pub(super) async fn emit_conversation_list_changed_event(
    state: &AppState,
    ctx: &PlatformRequestContext,
    conversation_id: Uuid,
    action: &str,
    source: &str,
) -> Result<(), AppError> {
    let mut tx = state.connect_pool.begin().await?;
    let event = event_store::insert_event_tx(
        &mut tx,
        ctx.tenant_id,
        conversation_id,
        None,
        RunEventInput {
            event_id: Some(format!(
                "conversation.listChanged.{action}.{}",
                Uuid::new_v4()
            )),
            event_type: "conversation.listChanged".to_string(),
            payload: Some(json!({
                "conversation_id": conversation_id.to_string(),
                "action": action,
                "source": source,
            })),
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
