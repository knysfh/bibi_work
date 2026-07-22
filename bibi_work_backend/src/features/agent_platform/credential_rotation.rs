use std::time::Duration;

use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use tokio::time::MissedTickBehavior;
use tracing::{debug, warn};
use uuid::Uuid;

use crate::{
    configuration::CredentialRotationSettings,
    features::{
        agent_platform::{
            audit::{NewAuditLog, insert_audit_log_tx},
            secret_resolver::{RotatedSecretRef, revoke_runtime_credentials_for_credential},
        },
        core::errors::AppError,
    },
    startup::AppState,
};

#[derive(Debug, Clone)]
struct RotationClaim {
    attempt_id: Uuid,
    credential_id: Uuid,
    tenant_id: Uuid,
    provider_key: String,
    secret_ref: String,
    rotation_interval_seconds: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CredentialRotationSummary {
    pub claimed: usize,
    pub succeeded: usize,
    pub failed: usize,
}

pub fn spawn_credential_rotation_worker(state: AppState, settings: CredentialRotationSettings) {
    if !settings.worker_enabled {
        return;
    }
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(
            settings.worker_interval_milliseconds(),
        ));
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            match process_due_credential_rotations(&state, &settings).await {
                Ok(summary) if summary.claimed > 0 => debug!(
                    claimed = summary.claimed,
                    succeeded = summary.succeeded,
                    failed = summary.failed,
                    "credential rotation worker completed"
                ),
                Ok(_) => {}
                Err(error) => warn!("credential rotation worker failed: {}", error),
            }
        }
    });
}

pub async fn process_due_credential_rotations(
    state: &AppState,
    settings: &CredentialRotationSettings,
) -> Result<CredentialRotationSummary, AppError> {
    let claims = claim_due_rotations(
        &state.connect_pool,
        settings.batch_size(),
        settings.stale_claim_seconds(),
    )
    .await?;
    let mut summary = CredentialRotationSummary {
        claimed: claims.len(),
        succeeded: 0,
        failed: 0,
    };
    for claim in claims {
        match rotate_claim(state, &claim).await {
            Ok(()) => summary.succeeded += 1,
            Err(error) => {
                summary.failed += 1;
                record_rotation_failure(&state.connect_pool, &claim, &error.to_string()).await?;
            }
        }
    }
    Ok(summary)
}

async fn claim_due_rotations(
    pool: &PgPool,
    limit: i64,
    stale_claim_seconds: i64,
) -> Result<Vec<RotationClaim>, AppError> {
    let mut tx = pool.begin().await?;
    let rows = sqlx::query(
        r#"
        WITH candidates AS (
            SELECT credential.id
            FROM llm_credentials credential
            WHERE credential.auto_rotation_enabled
              AND credential.revoked_at IS NULL
              AND credential.rotation_status = 'active'
              AND credential.secret_ref NOT LIKE 'local://%'
              AND (
                  credential.next_rotation_at <= CURRENT_TIMESTAMP
                  OR (
                      credential.expires_at IS NOT NULL
                      AND credential.expires_at <= CURRENT_TIMESTAMP
                          + credential.rotate_before_seconds * INTERVAL '1 second'
                  )
              )
              AND (
                  credential.rotation_started_at IS NULL
                  OR credential.rotation_started_at < CURRENT_TIMESTAMP - $2 * INTERVAL '1 second'
              )
            ORDER BY credential.next_rotation_at, credential.id
            FOR UPDATE SKIP LOCKED
            LIMIT $1
        )
        UPDATE llm_credentials credential
        SET rotation_started_at = CURRENT_TIMESTAMP,
            rotation_claim_id = COALESCE(credential.rotation_claim_id, gen_random_uuid()),
            updated_at = CURRENT_TIMESTAMP
        FROM candidates, llm_providers provider
        WHERE credential.id = candidates.id
          AND provider.id = credential.provider_id
        RETURNING credential.rotation_claim_id AS attempt_id,
                  credential.id AS credential_id, credential.tenant_id,
                  provider.provider_key, credential.secret_ref,
                  credential.rotation_interval_seconds
        "#,
    )
    .bind(limit)
    .bind(stale_claim_seconds)
    .fetch_all(&mut *tx)
    .await?;
    let mut claims = Vec::with_capacity(rows.len());
    for row in rows {
        let claim = RotationClaim {
            attempt_id: row.try_get("attempt_id")?,
            credential_id: row.try_get("credential_id")?,
            tenant_id: row.try_get("tenant_id")?,
            provider_key: row.try_get("provider_key")?,
            secret_ref: row.try_get("secret_ref")?,
            rotation_interval_seconds: row.try_get("rotation_interval_seconds")?,
        };
        sqlx::query(
            r#"
            INSERT INTO llm_credential_rotation_attempts (
                id, tenant_id, credential_id, status, resolver_scheme, previous_ref_hash
            ) VALUES ($1, $2, $3, 'running', $4, $5)
            ON CONFLICT (id) DO NOTHING
            "#,
        )
        .bind(claim.attempt_id)
        .bind(claim.tenant_id)
        .bind(claim.credential_id)
        .bind(secret_ref_scheme(&claim.secret_ref))
        .bind(secret_ref_hash(&claim.secret_ref))
        .execute(&mut *tx)
        .await?;
        claims.push(claim);
    }
    tx.commit().await?;
    Ok(claims)
}

async fn rotate_claim(state: &AppState, claim: &RotationClaim) -> Result<(), AppError> {
    revoke_runtime_credentials_for_credential(state, claim.credential_id).await?;
    let rotated = state
        .secret_resolver
        .rotate_secret_ref(
            claim.attempt_id,
            claim.tenant_id,
            claim.credential_id,
            &claim.provider_key,
            &claim.secret_ref,
        )
        .await?;
    persist_rotation_success(&state.connect_pool, claim, rotated).await
}

async fn persist_rotation_success(
    pool: &PgPool,
    claim: &RotationClaim,
    rotated: RotatedSecretRef,
) -> Result<(), AppError> {
    let new_ref_hash = secret_ref_hash(&rotated.secret_ref);
    let new_scheme = secret_ref_scheme(&rotated.secret_ref);
    let resource_id = claim.credential_id.to_string();
    let mut tx = pool.begin().await?;
    let updated = sqlx::query(
        r#"
        UPDATE llm_credentials
        SET secret_ref = $3,
            secret_hash = $4,
            expires_at = $5,
            last_rotated_at = CURRENT_TIMESTAMP,
            rotated_by_user_id = NULL,
            next_rotation_at = CURRENT_TIMESTAMP + $6 * INTERVAL '1 second',
            rotation_started_at = NULL,
            rotation_claim_id = NULL,
            rotation_attempts = 0,
            rotation_error = NULL,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1 AND tenant_id = $2 AND rotation_claim_id = $7
        "#,
    )
    .bind(claim.credential_id)
    .bind(claim.tenant_id)
    .bind(&rotated.secret_ref)
    .bind(rotated.secret_hash)
    .bind(rotated.expires_at)
    .bind(claim.rotation_interval_seconds)
    .bind(claim.attempt_id)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if updated != 1 {
        return Err(AppError::Conflict(
            "credential rotation claim is no longer active".to_string(),
        ));
    }
    sqlx::query(
        r#"
        UPDATE llm_credential_rotation_attempts
        SET status = 'succeeded', new_ref_hash = $2,
            completed_at = CURRENT_TIMESTAMP
        WHERE id = $1 AND status = 'running'
        "#,
    )
    .bind(claim.attempt_id)
    .bind(new_ref_hash)
    .execute(&mut *tx)
    .await?;
    insert_audit_log_tx(
        &mut tx,
        NewAuditLog {
            tenant_id: claim.tenant_id,
            actor_user_id: None,
            actor_device_id: None,
            session_id: None,
            resource_type: "llm_credential",
            resource_id: &resource_id,
            action: "rotate_automatic",
            decision: "allow",
            policy_version: "credential-rotation-v1",
            reason_code: Some(new_scheme),
            run_id: None,
            conversation_id: None,
            workflow_run_id: None,
            tool_call_id: None,
            approval_id: None,
            args_hash: None,
            input_summary: Some("rotation gateway returned a new opaque reference"),
            output_summary: Some("credential rotated and runtime credentials revoked"),
            risk_level: Some("high"),
            ip: None,
            user_agent: None,
            trace_id: None,
        },
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

async fn record_rotation_failure(
    pool: &PgPool,
    claim: &RotationClaim,
    error: &str,
) -> Result<(), AppError> {
    let error = bounded_error(error);
    let resource_id = claim.credential_id.to_string();
    let mut tx = pool.begin().await?;
    sqlx::query(
        r#"
        UPDATE llm_credentials
        SET rotation_started_at = NULL,
            rotation_claim_id = NULL,
            rotation_attempts = rotation_attempts + 1,
            rotation_error = $4,
            next_rotation_at = CURRENT_TIMESTAMP
                + LEAST(3600, 60 * power(2, LEAST(rotation_attempts, 5))) * INTERVAL '1 second',
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1 AND tenant_id = $2 AND rotation_claim_id = $3
        "#,
    )
    .bind(claim.credential_id)
    .bind(claim.tenant_id)
    .bind(claim.attempt_id)
    .bind(&error)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        r#"
        UPDATE llm_credential_rotation_attempts
        SET status = 'failed', error_summary = $2, completed_at = CURRENT_TIMESTAMP
        WHERE id = $1 AND status = 'running'
        "#,
    )
    .bind(claim.attempt_id)
    .bind(&error)
    .execute(&mut *tx)
    .await?;
    insert_audit_log_tx(
        &mut tx,
        NewAuditLog {
            tenant_id: claim.tenant_id,
            actor_user_id: None,
            actor_device_id: None,
            session_id: None,
            resource_type: "llm_credential",
            resource_id: &resource_id,
            action: "rotate_automatic",
            decision: "error",
            policy_version: "credential-rotation-v1",
            reason_code: Some("rotation_failed"),
            run_id: None,
            conversation_id: None,
            workflow_run_id: None,
            tool_call_id: None,
            approval_id: None,
            args_hash: None,
            input_summary: Some("automatic credential rotation failed"),
            output_summary: Some(&error),
            risk_level: Some("high"),
            ip: None,
            user_agent: None,
            trace_id: None,
        },
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

fn secret_ref_hash(value: &str) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(value.as_bytes())))
}

fn secret_ref_scheme(value: &str) -> &'static str {
    if value.starts_with("vault://") {
        "vault"
    } else if value.starts_with("kms://") {
        "kms"
    } else {
        "env"
    }
}

fn bounded_error(value: &str) -> String {
    value.chars().take(2_000).collect()
}

#[cfg(test)]
mod tests {
    use axum::{Json, Router, http::HeaderMap, routing::post};
    use redis::Client as RedisClient;
    use secrecy::SecretBox;
    use serde_json::{Value, json};
    use sqlx::postgres::PgPoolOptions;
    use tokio::net::TcpListener;

    use super::*;
    use crate::{
        configuration::{
            AgentRuntimeSettings, FerrisKeySettings, MemoryVectorSettings, SecretResolverSettings,
        },
        features::agent_platform::{
            authz::ResourceAuthzService, ferriskey_oidc::FerrisKeyOidcVerifier,
            memory_vector::MemoryVectorClient, runtime::AgentRuntimeClient, rustfs::RustFsClient,
            secret_resolver::SecretResolver,
        },
    };

    #[test]
    fn rotation_attempt_metadata_never_uses_raw_secret_refs() {
        let value = "vault://secret/data/tenant/provider#api_key";
        let hash = secret_ref_hash(value);
        assert!(hash.starts_with("sha256:"));
        assert!(!hash.contains("vault"));
        assert_eq!(secret_ref_scheme(value), "vault");
        assert_eq!(bounded_error(&"x".repeat(3_000)).len(), 2_000);
    }

    #[tokio::test]
    #[ignore = "requires disposable local Postgres and Redis"]
    async fn worker_rotates_due_credential_and_records_audit_without_raw_refs()
    -> Result<(), Box<dyn std::error::Error>> {
        unsafe {
            std::env::set_var("BIBI_TEST_AUTO_ROTATION_TOKEN", "rotation-token");
            std::env::set_var("BIBI_TEST_ROTATED_LLM_KEY", "rotated-value");
        }
        async fn rotate(headers: HeaderMap, Json(payload): Json<Value>) -> Json<Value> {
            assert_eq!(
                headers
                    .get("authorization")
                    .and_then(|value| value.to_str().ok()),
                Some("Bearer rotation-token")
            );
            assert_eq!(payload["provider_key"], "rotation-test-provider");
            Json(json!({
                "secret_ref": "env://BIBI_TEST_ROTATED_LLM_KEY",
                "secret_hash": "sha256:rotated"
            }))
        }
        let router = Router::new().route("/rotate", post(rotate));
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let base_url = format!("http://{}", listener.local_addr()?);
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });

        let database_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
            "postgresql://postgres:password@127.0.0.1:5433/bibi_work".to_string()
        });
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(&database_url)
            .await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        let tenant_id = Uuid::new_v4();
        sqlx::query("INSERT INTO tenants (id, name, slug) VALUES ($1, 'Rotation test', $2)")
            .bind(tenant_id)
            .bind(format!("rotation-test-{tenant_id}"))
            .execute(&pool)
            .await?;
        let provider_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO llm_providers (tenant_id, provider_key, display_name)
            VALUES ($1, 'rotation-test-provider', 'Rotation Test Provider') RETURNING id
            "#,
        )
        .bind(tenant_id)
        .fetch_one(&pool)
        .await?;
        let stale_attempt_id = Uuid::new_v4();
        let credential_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO llm_credentials (
                tenant_id, provider_id, secret_ref, auto_rotation_enabled,
                rotation_interval_seconds, rotate_before_seconds, next_rotation_at,
                rotation_started_at, rotation_claim_id
            ) VALUES ($1, $2, 'env://BIBI_TEST_OLD_LLM_KEY', TRUE, 3600, 300,
                      CURRENT_TIMESTAMP - INTERVAL '1 minute',
                      CURRENT_TIMESTAMP - INTERVAL '2 minutes', $3)
            RETURNING id
            "#,
        )
        .bind(tenant_id)
        .bind(provider_id)
        .bind(stale_attempt_id)
        .fetch_one(&pool)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO llm_credential_rotation_attempts (
                id, tenant_id, credential_id, status, resolver_scheme, previous_ref_hash
            ) VALUES ($1, $2, $3, 'running', 'env', $4)
            "#,
        )
        .bind(stale_attempt_id)
        .bind(tenant_id)
        .bind(credential_id)
        .bind(secret_ref_hash("env://BIBI_TEST_OLD_LLM_KEY"))
        .execute(&pool)
        .await?;
        let resolver = SecretResolver::new(SecretResolverSettings {
            timeout_milliseconds: 2_000,
            vault_enabled: false,
            vault_base_url: None,
            vault_token_ref: None,
            vault_namespace: None,
            kms_enabled: false,
            kms_base_url: None,
            kms_auth_token_ref: None,
            rotation_gateway_enabled: true,
            rotation_gateway_base_url: Some(base_url),
            rotation_gateway_auth_token_ref: Some(
                "env://BIBI_TEST_AUTO_ROTATION_TOKEN".to_string(),
            ),
        })?;
        let state = AppState {
            connect_pool: pool.clone(),
            redis_client: RedisClient::open("redis://127.0.0.1:6380")?,
            ferriskey_oidc: FerrisKeyOidcVerifier::new(FerrisKeySettings {
                issuer: "http://localhost:3333/realms/bibi-work".to_string(),
                audience: "bibi-work-backend".to_string(),
                trusted_authorized_parties: Vec::new(),
                discovery_url:
                    "http://localhost:3333/realms/bibi-work/.well-known/openid-configuration"
                        .to_string(),
                jwks_uri: None,
                default_tenant_slug: "bibi-work".to_string(),
                timeout_milliseconds: 1_000,
            })?,
            authz_service: ResourceAuthzService::new(pool.clone()),
            agent_runtime_client: AgentRuntimeClient::new(AgentRuntimeSettings {
                base_url: None,
                shared_token: secret("test-token"),
                timeout_milliseconds: 1_000,
            })?,
            rustfs_client: RustFsClient::disabled_for_tests(),
            memory_vector_client: MemoryVectorClient::new(MemoryVectorSettings {
                enabled: false,
                embedding_endpoint: None,
                qdrant_rest_url: None,
                qdrant_collection: "rotation_test".to_string(),
                timeout_milliseconds: 1_000,
                max_context_chars: 1_200,
                worker_interval_milliseconds: 1_000,
                worker_batch_size: 1,
                worker_max_attempts: 1,
            })?,
            internal_shared_token: "test-token".to_string(),
            audit_partition_cleanup_enabled: false,
            secret_resolver: resolver,
            credential_rotation_worker_enabled: true,
        };
        let summary = process_due_credential_rotations(
            &state,
            &CredentialRotationSettings {
                worker_enabled: true,
                worker_interval_milliseconds: 1_000,
                batch_size: 10,
                stale_claim_seconds: 60,
            },
        )
        .await?;
        assert_eq!(summary.succeeded, 1);
        let row = sqlx::query(
            "SELECT secret_ref, rotation_error, rotation_started_at FROM llm_credentials WHERE id = $1",
        )
        .bind(credential_id)
        .fetch_one(&pool)
        .await?;
        assert_eq!(
            row.try_get::<String, _>("secret_ref")?,
            "env://BIBI_TEST_ROTATED_LLM_KEY"
        );
        assert!(
            row.try_get::<Option<String>, _>("rotation_error")?
                .is_none()
        );
        assert!(
            row.try_get::<Option<time::OffsetDateTime>, _>("rotation_started_at")?
                .is_none()
        );
        let attempt_row = sqlx::query(
            "SELECT id, status FROM llm_credential_rotation_attempts WHERE credential_id = $1",
        )
        .bind(credential_id)
        .fetch_one(&pool)
        .await?;
        assert_eq!(attempt_row.try_get::<Uuid, _>("id")?, stale_attempt_id);
        assert_eq!(attempt_row.try_get::<String, _>("status")?, "succeeded");
        let leaked: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*) FROM audit_logs
            WHERE resource_type = 'llm_credential' AND resource_id = $1
              AND concat_ws(' ', input_summary, output_summary, reason_code) ~* '(env://|vault://|kms://)'
            "#,
        )
        .bind(credential_id.to_string())
        .fetch_one(&pool)
        .await?;
        assert_eq!(leaked, 0);

        let failed_credential_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO llm_credentials (
                tenant_id, provider_id, secret_ref, auto_rotation_enabled,
                rotation_interval_seconds, rotate_before_seconds, next_rotation_at
            ) VALUES ($1, $2, 'env://BIBI_TEST_OLD_LLM_KEY', TRUE, 3600, 300,
                      CURRENT_TIMESTAMP - INTERVAL '1 minute')
            RETURNING id
            "#,
        )
        .bind(tenant_id)
        .bind(provider_id)
        .fetch_one(&pool)
        .await?;
        let mut failure_state = state.clone();
        failure_state.secret_resolver = SecretResolver::env_only_for_tests();
        let failed_summary = process_due_credential_rotations(
            &failure_state,
            &CredentialRotationSettings {
                worker_enabled: true,
                worker_interval_milliseconds: 1_000,
                batch_size: 10,
                stale_claim_seconds: 60,
            },
        )
        .await?;
        assert_eq!(failed_summary.failed, 1);
        let failed_row = sqlx::query(
            "SELECT secret_ref, rotation_error, rotation_started_at, rotation_status FROM llm_credentials WHERE id = $1",
        )
        .bind(failed_credential_id)
        .fetch_one(&pool)
        .await?;
        assert_eq!(
            failed_row.try_get::<String, _>("secret_ref")?,
            "env://BIBI_TEST_OLD_LLM_KEY"
        );
        assert_eq!(
            failed_row.try_get::<String, _>("rotation_status")?,
            "active"
        );
        assert!(
            failed_row
                .try_get::<Option<String>, _>("rotation_error")?
                .is_some()
        );
        assert!(
            failed_row
                .try_get::<Option<time::OffsetDateTime>, _>("rotation_started_at")?
                .is_none()
        );
        sqlx::query("DELETE FROM tenants WHERE id = $1")
            .bind(tenant_id)
            .execute(&pool)
            .await?;
        server.abort();
        Ok(())
    }

    fn secret(value: &str) -> SecretBox<str> {
        SecretBox::new(value.to_string().into_boxed_str())
    }
}
