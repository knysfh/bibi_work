use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::{features::core::errors::AppError, startup::AppState};

pub(super) async fn disable_extension_connectors(
    state: &AppState,
    tenant_id: Uuid,
    extension_package_id: Uuid,
) -> Result<u64, AppError> {
    Ok(sqlx::query(
        r#"
        UPDATE channel_connectors
        SET enabled = FALSE,
            connected = FALSE,
            status = 'disabled',
            updated_at = CURRENT_TIMESTAMP
        WHERE tenant_id = $1 AND source_extension_package_id = $2
        "#,
    )
    .bind(tenant_id)
    .bind(extension_package_id)
    .execute(&state.connect_pool)
    .await?
    .rows_affected())
}

pub(super) async fn disable_extension_connectors_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    extension_package_id: Uuid,
) -> Result<u64, AppError> {
    Ok(sqlx::query(
        r#"
        UPDATE channel_connectors
        SET enabled = FALSE,
            connected = FALSE,
            status = 'disabled',
            updated_at = CURRENT_TIMESTAMP
        WHERE tenant_id = $1 AND source_extension_package_id = $2
        "#,
    )
    .bind(tenant_id)
    .bind(extension_package_id)
    .execute(&mut **tx)
    .await?
    .rows_affected())
}
