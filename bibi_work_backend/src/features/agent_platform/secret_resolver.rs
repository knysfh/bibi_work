use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::Row;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::{features::core::errors::AppError, startup::AppState};

const RUNTIME_CREDENTIAL_TTL_SECONDS: i64 = 600;

#[derive(Debug)]
struct LlmCredentialSecret {
    credential_id: Uuid,
    provider_key: String,
    auth_scheme: String,
    secret_ref: String,
}

#[derive(Deserialize, Serialize)]
pub struct RuntimeCredential {
    pub tenant_id: Uuid,
    pub run_id: Uuid,
    pub credential_id: Uuid,
    pub provider_key: String,
    pub auth_scheme: String,
    pub secret: String,
    pub expires_at: OffsetDateTime,
}

pub async fn attach_llm_runtime_credential(
    state: &AppState,
    tenant_id: Uuid,
    run_id: Uuid,
    snapshot: &mut Value,
) -> Result<(), AppError> {
    let Some(credential_id) = model_credential_id(snapshot)? else {
        return Ok(());
    };
    let secret = load_llm_credential_secret(state, tenant_id, credential_id).await?;
    let secret_value = resolve_secret_ref(&secret.secret_ref)?;
    let runtime_credential_id =
        issue_runtime_credential(state, tenant_id, run_id, secret, secret_value).await?;
    set_runtime_credential_id(snapshot.get_mut("model"), &runtime_credential_id);
    set_runtime_credential_id(snapshot.pointer_mut("/agent/model"), &runtime_credential_id);
    Ok(())
}

pub async fn load_runtime_credential(
    state: &AppState,
    tenant_id: Uuid,
    run_id: Uuid,
    runtime_credential_id: &str,
) -> Result<RuntimeCredential, AppError> {
    let mut conn = state
        .redis_client
        .get_multiplexed_async_connection()
        .await?;
    let payload: Option<String> = redis::cmd("GET")
        .arg(runtime_credential_key(runtime_credential_id))
        .query_async(&mut conn)
        .await?;
    let Some(payload) = payload else {
        return Err(AppError::NotFound(
            "runtime credential not found or expired".to_string(),
        ));
    };
    let credential: RuntimeCredential = serde_json::from_str(&payload)
        .map_err(|_| AppError::InvalidInput("runtime credential payload is invalid".to_string()))?;
    if credential.tenant_id != tenant_id || credential.run_id != run_id {
        return Err(AppError::PermissionDenied(
            "runtime credential scope mismatch".to_string(),
        ));
    }
    if credential.expires_at <= OffsetDateTime::now_utc() {
        return Err(AppError::NotFound(
            "runtime credential not found or expired".to_string(),
        ));
    }
    Ok(credential)
}

async fn load_llm_credential_secret(
    state: &AppState,
    tenant_id: Uuid,
    credential_id: Uuid,
) -> Result<LlmCredentialSecret, AppError> {
    let row = sqlx::query(
        r#"
        SELECT c.id AS credential_id, c.secret_ref, p.provider_key, p.auth_scheme
        FROM llm_credentials c
        JOIN llm_providers p ON p.id = c.provider_id
        WHERE c.id = $1
          AND c.tenant_id = $2
          AND c.revoked_at IS NULL
          AND c.rotation_status = 'active'
          AND (c.expires_at IS NULL OR c.expires_at > CURRENT_TIMESTAMP)
          AND p.status = 'active'
        "#,
    )
    .bind(credential_id)
    .bind(tenant_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::InvalidInput("llm credential is not active".to_string()))?;

    Ok(LlmCredentialSecret {
        credential_id: row.try_get("credential_id")?,
        provider_key: row.try_get("provider_key")?,
        auth_scheme: row.try_get("auth_scheme")?,
        secret_ref: row.try_get("secret_ref")?,
    })
}

async fn issue_runtime_credential(
    state: &AppState,
    tenant_id: Uuid,
    run_id: Uuid,
    credential: LlmCredentialSecret,
    secret: String,
) -> Result<String, AppError> {
    let runtime_credential_id = Uuid::new_v4().to_string();
    let expires_at = OffsetDateTime::now_utc() + Duration::seconds(RUNTIME_CREDENTIAL_TTL_SECONDS);
    let payload = RuntimeCredential {
        tenant_id,
        run_id,
        credential_id: credential.credential_id,
        provider_key: credential.provider_key,
        auth_scheme: credential.auth_scheme,
        secret,
        expires_at,
    };
    let encoded = serde_json::to_string(&payload)
        .map_err(|_| AppError::InvalidInput("failed to encode runtime credential".to_string()))?;
    let mut conn = state
        .redis_client
        .get_multiplexed_async_connection()
        .await?;
    let _: String = redis::cmd("SETEX")
        .arg(runtime_credential_key(&runtime_credential_id))
        .arg(RUNTIME_CREDENTIAL_TTL_SECONDS)
        .arg(encoded)
        .query_async(&mut conn)
        .await?;
    Ok(runtime_credential_id)
}

fn model_credential_id(snapshot: &Value) -> Result<Option<Uuid>, AppError> {
    credential_id_from_model(snapshot.get("model"))
        .or_else(|| credential_id_from_model(snapshot.pointer("/agent/model")))
        .transpose()
}

fn credential_id_from_model(model: Option<&Value>) -> Option<Result<Uuid, AppError>> {
    let model = model?;
    let has_secret_ref = model
        .pointer("/credential/has_secret_ref")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !has_secret_ref {
        return None;
    }
    let value = model
        .pointer("/credential/credential_id")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            AppError::InvalidInput(
                "model credential requires credential_id when has_secret_ref is true".to_string(),
            )
        });
    Some(value.and_then(|value| {
        Uuid::parse_str(value)
            .map_err(|_| AppError::InvalidInput("model credential_id must be a uuid".to_string()))
    }))
}

fn set_runtime_credential_id(model: Option<&mut Value>, runtime_credential_id: &str) {
    let Some(model) = model else {
        return;
    };
    let Some(credential) = model.get_mut("credential").and_then(Value::as_object_mut) else {
        return;
    };
    credential.insert(
        "runtime_credential_id".to_string(),
        Value::String(runtime_credential_id.to_string()),
    );
}

pub fn resolve_secret_ref(secret_ref: &str) -> Result<String, AppError> {
    let env_name = env_name_from_secret_ref(secret_ref)?;
    std::env::var(env_name).map_err(|_| {
        AppError::InvalidInput("secret_ref points to a missing environment variable".to_string())
    })
}

pub fn env_name_from_secret_ref(secret_ref: &str) -> Result<&str, AppError> {
    let env_name = secret_ref
        .strip_prefix("env://")
        .or_else(|| secret_ref.strip_prefix("env:"))
        .ok_or_else(|| {
            AppError::InvalidInput(
                "unsupported secret_ref scheme; supported scheme is env://".to_string(),
            )
        })?;
    if env_name.is_empty()
        || env_name.len() > 128
        || !env_name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(AppError::InvalidInput(
            "env secret_ref must name one environment variable".to_string(),
        ));
    }
    Ok(env_name)
}

fn runtime_credential_key(runtime_credential_id: &str) -> String {
    format!("bibi:runtime-credential:{runtime_credential_id}")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn env_secret_ref_accepts_single_environment_variable() {
        assert_eq!(
            env_name_from_secret_ref("env://OPENAI_API_KEY").unwrap(),
            "OPENAI_API_KEY"
        );
        assert_eq!(
            env_name_from_secret_ref("env:ANTHROPIC_KEY").unwrap(),
            "ANTHROPIC_KEY"
        );
        assert!(env_name_from_secret_ref("vault://tenant/key").is_err());
        assert!(env_name_from_secret_ref("env://TENANT/KEY").is_err());
    }

    #[test]
    fn model_credential_id_requires_uuid_when_secret_backed() {
        assert!(
            model_credential_id(&json!({"model": {"credential": {"has_secret_ref": false}}}))
                .unwrap()
                .is_none()
        );
        assert!(
            model_credential_id(&json!({"model": {"credential": {"has_secret_ref": true}}}))
                .is_err()
        );
    }
}
