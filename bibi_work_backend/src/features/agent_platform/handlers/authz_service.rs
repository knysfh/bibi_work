use axum::{Extension, Json, extract::State};

use crate::{
    features::{
        agent_platform::{ferriskey_oidc::PlatformRequestContext, models::*},
        core::errors::AppError,
    },
    startup::AppState,
};

use super::support::*;

pub async fn api_authz_check(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<AuthzCheckRequest>,
) -> Result<Json<AuthzDecision>, AppError> {
    let mut check = payload;
    let actor_matches = normalize_public_authz_actor_for_audit(&mut check, &ctx);
    let decision = if !actor_matches {
        AuthzDecision::deny("request-validation", "actor_mismatch")
    } else {
        state.authz_service.check(&check).await
    };

    write_authz_audit(&state.connect_pool, &check, &decision).await?;
    Ok(Json(decision))
}

pub async fn api_authz_batch_check(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<AuthzBatchCheckRequest>,
) -> Result<Json<AuthzBatchCheckResponse>, AppError> {
    let mut decisions = Vec::with_capacity(payload.checks.len());
    for mut check in payload.checks {
        let actor_matches = normalize_public_authz_actor_for_audit(&mut check, &ctx);
        let decision = if !actor_matches {
            AuthzDecision::deny("request-validation", "actor_mismatch")
        } else {
            state.authz_service.check(&check).await
        };
        write_authz_audit(&state.connect_pool, &check, &decision).await?;
        decisions.push(decision);
    }

    Ok(Json(AuthzBatchCheckResponse { decisions }))
}

fn normalize_public_authz_actor_for_audit(
    check: &mut AuthzCheckRequest,
    ctx: &PlatformRequestContext,
) -> bool {
    let actor_matches = check.actor.user_id == ctx.platform_user_id;
    if !actor_matches {
        check.actor.user_id = ctx.platform_user_id;
        check.context = None;
    }
    normalize_request_actor(check, ctx);
    actor_matches
}

pub async fn internal_authz_check(
    State(state): State<AppState>,
    Json(payload): Json<AuthzCheckRequest>,
) -> Result<Json<AuthzDecision>, AppError> {
    let decision = state.authz_service.check(&payload).await;
    write_authz_audit(&state.connect_pool, &payload, &decision).await?;
    Ok(Json(decision))
}

pub async fn internal_authz_batch_check(
    State(state): State<AppState>,
    Json(payload): Json<AuthzBatchCheckRequest>,
) -> Result<Json<AuthzBatchCheckResponse>, AppError> {
    let response = state.authz_service.batch_check(&payload).await;
    for (check, decision) in payload.checks.iter().zip(response.decisions.iter()) {
        write_authz_audit(&state.connect_pool, check, decision).await?;
    }
    Ok(Json(response))
}

#[cfg(test)]
mod tests {
    use super::*;
    use redis::Client as RedisClient;
    use secrecy::SecretBox;
    use serde_json::json;
    use sqlx::{PgPool, Row, postgres::PgPoolOptions};
    use uuid::Uuid;

    use crate::{
        configuration::{AgentRuntimeSettings, FerrisKeySettings, MemoryVectorSettings},
        features::agent_platform::{
            authz::ResourceAuthzService, ferriskey_oidc::FerrisKeyOidcVerifier,
            memory_vector::MemoryVectorClient, runtime::AgentRuntimeClient, rustfs::RustFsClient,
        },
    };

    struct PublicAuthzTestContext {
        tenant_id: Uuid,
        user_id: Uuid,
        device_id: Uuid,
        session_id: Uuid,
        platform_context: PlatformRequestContext,
    }

    #[tokio::test]
    #[ignore]
    async fn public_authz_check_actor_mismatch_denies_and_audits_current_actor()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = test_state().await?;
        let context = seed_public_authz_context(&state.connect_pool).await?;
        let spoofed_user_id = Uuid::new_v4();

        let Json(decision) = api_authz_check(
            State(state.clone()),
            Extension(context.platform_context.clone()),
            Json(authz_request(
                context.tenant_id,
                spoofed_user_id,
                "read",
                "project",
                Uuid::new_v4().to_string(),
            )),
        )
        .await?;

        assert_eq!(decision.decision, "deny");
        assert_eq!(decision.policy_version, "request-validation");
        assert_eq!(decision.reason_code.as_deref(), Some("actor_mismatch"));
        assert_no_authz_rows_for_actor(&state.connect_pool, context.tenant_id, spoofed_user_id)
            .await?;
        assert_latest_authz_row(
            &state.connect_pool,
            context.tenant_id,
            context.user_id,
            "deny",
            "actor_mismatch",
            "trace-authz",
        )
        .await?;
        assert_latest_audit_row(
            &state.connect_pool,
            context.tenant_id,
            context.user_id,
            "deny",
            "actor_mismatch",
            "trace-authz",
        )
        .await?;

        cleanup_tenant(&state.connect_pool, context.tenant_id).await?;
        Ok(())
    }

    #[tokio::test]
    #[ignore]
    async fn public_authz_batch_check_mixes_actor_mismatch_with_real_actor_decision()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = test_state().await?;
        let context = seed_public_authz_context(&state.connect_pool).await?;
        let spoofed_user_id = Uuid::new_v4();
        let project_id = Uuid::new_v4().to_string();

        let Json(response) = api_authz_batch_check(
            State(state.clone()),
            Extension(context.platform_context.clone()),
            Json(AuthzBatchCheckRequest {
                checks: vec![
                    authz_request(
                        context.tenant_id,
                        spoofed_user_id,
                        "read",
                        "project",
                        project_id.clone(),
                    ),
                    authz_request(
                        context.tenant_id,
                        context.user_id,
                        "read",
                        "project",
                        project_id,
                    ),
                ],
            }),
        )
        .await?;

        assert_eq!(response.decisions.len(), 2);
        assert_eq!(response.decisions[0].decision, "deny");
        assert_eq!(
            response.decisions[0].reason_code.as_deref(),
            Some("actor_mismatch")
        );
        assert_eq!(response.decisions[1].decision, "allow");
        assert_no_authz_rows_for_actor(&state.connect_pool, context.tenant_id, spoofed_user_id)
            .await?;

        let rows = sqlx::query(
            r#"
            SELECT decision, reason_code, actor_user_id, actor_device_id, session_id,
                   context->>'trace_id' AS trace_id
            FROM authz_decisions
            WHERE tenant_id = $1
            ORDER BY created_at DESC
            LIMIT 2
            "#,
        )
        .bind(context.tenant_id)
        .fetch_all(&state.connect_pool)
        .await?;
        assert_eq!(rows.len(), 2);
        let mut saw_mismatch = false;
        let mut saw_allow = false;
        for row in rows {
            assert_eq!(row.try_get::<Uuid, _>("actor_user_id")?, context.user_id);
            assert_eq!(
                row.try_get::<Uuid, _>("actor_device_id")?,
                context.device_id
            );
            assert_eq!(row.try_get::<Uuid, _>("session_id")?, context.session_id);

            let decision: String = row.try_get("decision")?;
            let reason_code: Option<String> = row.try_get("reason_code")?;
            let trace_id: String = row.try_get("trace_id")?;
            if decision == "deny" && reason_code.as_deref() == Some("actor_mismatch") {
                saw_mismatch = true;
                assert_eq!(trace_id, "trace-authz");
            } else if decision == "allow" && reason_code.is_none() {
                saw_allow = true;
                assert_eq!(trace_id, "client-supplied-trace");
            } else {
                panic!("unexpected authz row decision={decision} reason={reason_code:?}");
            }
        }
        assert!(saw_mismatch);
        assert!(saw_allow);

        cleanup_tenant(&state.connect_pool, context.tenant_id).await?;
        Ok(())
    }

    fn authz_request(
        tenant_id: Uuid,
        actor_user_id: Uuid,
        action: &str,
        resource_type: &str,
        resource_id: String,
    ) -> AuthzCheckRequest {
        AuthzCheckRequest {
            tenant_id,
            actor: ActorRef {
                user_id: actor_user_id,
                device_id: None,
                session_id: None,
                roles: vec!["admin".to_string()],
            },
            action: action.to_string(),
            resource: ResourceRef {
                resource_type: resource_type.to_string(),
                id: resource_id,
                path: None,
            },
            context: Some(AuthzContext {
                trace_id: Some("client-supplied-trace".to_string()),
                run_id: Some(Uuid::new_v4()),
                ..Default::default()
            }),
        }
    }

    async fn seed_public_authz_context(
        pool: &PgPool,
    ) -> Result<PublicAuthzTestContext, sqlx::Error> {
        let tenant_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();
        let device_id = Uuid::new_v4();
        let session_id = Uuid::new_v4();
        sqlx::query("INSERT INTO tenants (id, name, slug) VALUES ($1, 'Authz Test', $2)")
            .bind(tenant_id)
            .bind(format!("authz-test-{tenant_id}"))
            .execute(pool)
            .await?;
        sqlx::query(
            r#"
            INSERT INTO platform_users (id, tenant_id, ferriskey_subject, username, status)
            VALUES ($1, $2, 'authz-subject', 'authz-user', 'active')
            "#,
        )
        .bind(user_id)
        .bind(tenant_id)
        .execute(pool)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO user_tenant_memberships (tenant_id, user_id, role)
            VALUES ($1, $2, 'member')
            "#,
        )
        .bind(tenant_id)
        .bind(user_id)
        .execute(pool)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO devices (
                id, tenant_id, user_id, device_fingerprint, device_name, platform, trust_level
            )
            VALUES ($1, $2, $3, 'authz-device', 'Authz Device', 'oidc', 'standard')
            "#,
        )
        .bind(device_id)
        .bind(tenant_id)
        .bind(user_id)
        .execute(pool)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO platform_sessions (
                id, tenant_id, user_id, device_id, ferriskey_subject, ferriskey_session_state,
                token_exp, roles_snapshot, token_hash
            )
            VALUES (
                $1, $2, $3, $4, 'authz-subject', 'authz-session',
                CURRENT_TIMESTAMP + INTERVAL '1 hour', $5, 'token-hash'
            )
            "#,
        )
        .bind(session_id)
        .bind(tenant_id)
        .bind(user_id)
        .bind(device_id)
        .bind(json!(["tenant_member"]))
        .execute(pool)
        .await?;

        let platform_context = PlatformRequestContext {
            tenant_id,
            platform_user_id: user_id,
            ferriskey_subject: "authz-subject".to_string(),
            preferred_username: Some("authz-user".to_string()),
            email: None,
            roles: vec!["tenant_member".to_string()],
            session_id,
            device_id,
            trace_id: "trace-authz".to_string(),
            token_jti: None,
            token_exp: time::OffsetDateTime::now_utc() + time::Duration::hours(1),
        };

        Ok(PublicAuthzTestContext {
            tenant_id,
            user_id,
            device_id,
            session_id,
            platform_context,
        })
    }

    async fn assert_no_authz_rows_for_actor(
        pool: &PgPool,
        tenant_id: Uuid,
        actor_user_id: Uuid,
    ) -> Result<(), sqlx::Error> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM authz_decisions WHERE tenant_id = $1 AND actor_user_id = $2",
        )
        .bind(tenant_id)
        .bind(actor_user_id)
        .fetch_one(pool)
        .await?;
        assert_eq!(count, 0);
        Ok(())
    }

    async fn assert_latest_authz_row(
        pool: &PgPool,
        tenant_id: Uuid,
        actor_user_id: Uuid,
        decision: &str,
        reason_code: &str,
        trace_id: &str,
    ) -> Result<(), sqlx::Error> {
        let row = sqlx::query(
            r#"
            SELECT actor_user_id, decision, reason_code, context->>'trace_id' AS trace_id
            FROM authz_decisions
            WHERE tenant_id = $1
            ORDER BY created_at DESC
            LIMIT 1
            "#,
        )
        .bind(tenant_id)
        .fetch_one(pool)
        .await?;
        assert_eq!(row.try_get::<Uuid, _>("actor_user_id")?, actor_user_id);
        assert_eq!(row.try_get::<String, _>("decision")?, decision);
        assert_eq!(row.try_get::<String, _>("reason_code")?, reason_code);
        assert_eq!(row.try_get::<String, _>("trace_id")?, trace_id);
        Ok(())
    }

    async fn assert_latest_audit_row(
        pool: &PgPool,
        tenant_id: Uuid,
        actor_user_id: Uuid,
        decision: &str,
        reason_code: &str,
        trace_id: &str,
    ) -> Result<(), sqlx::Error> {
        let row = sqlx::query(
            r#"
            SELECT actor_user_id, decision, reason_code, trace_id
            FROM audit_logs
            WHERE tenant_id = $1
            ORDER BY created_at DESC
            LIMIT 1
            "#,
        )
        .bind(tenant_id)
        .fetch_one(pool)
        .await?;
        assert_eq!(row.try_get::<Uuid, _>("actor_user_id")?, actor_user_id);
        assert_eq!(row.try_get::<String, _>("decision")?, decision);
        assert_eq!(row.try_get::<String, _>("reason_code")?, reason_code);
        assert_eq!(row.try_get::<String, _>("trace_id")?, trace_id);
        Ok(())
    }

    async fn test_pool() -> Result<PgPool, Box<dyn std::error::Error>> {
        let database_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
            "postgresql://postgres:password@127.0.0.1:5433/bibi_work".to_string()
        });
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(&database_url)
            .await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        Ok(pool)
    }

    async fn test_state() -> Result<AppState, Box<dyn std::error::Error>> {
        let pool = test_pool().await?;
        let redis_url =
            std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6380".to_string());

        Ok(AppState {
            connect_pool: pool.clone(),
            redis_client: RedisClient::open(redis_url)?,
            ferriskey_oidc: FerrisKeyOidcVerifier::new(FerrisKeySettings {
                issuer: "http://localhost:3333/realms/bibi-work".to_string(),
                audience: "bibi-work-backend".to_string(),
                trusted_authorized_parties: Vec::new(),
                discovery_url:
                    "http://localhost:3333/realms/bibi-work/.well-known/openid-configuration"
                        .to_string(),
                jwks_uri: None,
                default_tenant_slug: "bibi-work".to_string(),
                timeout_milliseconds: 1000,
            })?,
            authz_service: ResourceAuthzService::new(pool),
            agent_runtime_client: AgentRuntimeClient::new(AgentRuntimeSettings {
                base_url: None,
                shared_token: secret("test-internal-token"),
                timeout_milliseconds: 1000,
            })?,
            rustfs_client: RustFsClient::disabled_for_tests(),
            memory_vector_client: MemoryVectorClient::new(MemoryVectorSettings {
                enabled: false,
                embedding_endpoint: None,
                qdrant_rest_url: None,
                qdrant_collection: "test_memories".to_string(),
                timeout_milliseconds: 1000,
                max_context_chars: 1200,
                worker_interval_milliseconds: 1000,
                worker_batch_size: 1,
                worker_max_attempts: 1,
            })?,
            internal_shared_token: "test-internal-token".to_string(),
            audit_partition_cleanup_enabled: false,
            secret_resolver:
                crate::features::agent_platform::secret_resolver::SecretResolver::env_only_for_tests(
                ),
            credential_rotation_worker_enabled: false,
        })
    }

    async fn cleanup_tenant(pool: &PgPool, tenant_id: Uuid) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM tenants WHERE id = $1")
            .bind(tenant_id)
            .execute(pool)
            .await?;
        Ok(())
    }

    fn secret(value: &str) -> SecretBox<str> {
        SecretBox::new(value.to_string().into_boxed_str())
    }
}
