use serde_json::{Value, json};
use uuid::Uuid;

use crate::{features::core::errors::AppError, startup::AppState};

pub(super) fn parse_uuid_id(value: &str, label: &str) -> Result<Uuid, AppError> {
    Uuid::parse_str(value).map_err(|_| AppError::NotFound(format!("{label} not found")))
}

pub(super) fn merge_conversation_extra(extra: Value) -> Value {
    let mut object = extra.as_object().cloned().unwrap_or_default();
    object
        .entry("backend".to_string())
        .or_insert_with(|| json!("deepagents"));
    object
        .entry("workspace".to_string())
        .or_insert_with(|| json!(""));
    Value::Object(object)
}

pub(super) async fn ensure_conversation_exists(
    state: &AppState,
    tenant_id: Uuid,
    conversation_id: Uuid,
) -> Result<(), AppError> {
    let exists: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM conversations
            WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL
        )
        "#,
    )
    .bind(conversation_id)
    .bind(tenant_id)
    .fetch_one(&state.connect_pool)
    .await?;
    if exists {
        Ok(())
    } else {
        Err(AppError::NotFound("conversation not found".to_string()))
    }
}
