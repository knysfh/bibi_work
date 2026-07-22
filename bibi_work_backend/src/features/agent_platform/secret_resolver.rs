use std::time::Duration as StdDuration;

use base64::{Engine, engine::general_purpose::STANDARD};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::Row;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::{
    configuration::SecretResolverSettings,
    features::{
        agent_platform::audit::{NewAuditLog, insert_audit_log_tx},
        core::errors::AppError,
    },
    startup::AppState,
};

const RUNTIME_CREDENTIAL_TTL_SECONDS: i64 = 600;

#[derive(Clone)]
pub struct SecretResolver {
    http: Client,
    settings: SecretResolverSettings,
}

#[derive(Debug, Serialize)]
struct RotateSecretRefRequest<'a> {
    attempt_id: Uuid,
    tenant_id: Uuid,
    credential_id: Uuid,
    provider_key: &'a str,
    current_secret_ref: &'a str,
}

#[derive(Debug, Deserialize)]
struct RotateSecretRefResponse {
    secret_ref: String,
    secret_hash: Option<String>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    expires_at: Option<OffsetDateTime>,
}

#[derive(Debug)]
pub struct RotatedSecretRef {
    pub secret_ref: String,
    pub secret_hash: Option<String>,
    pub expires_at: Option<OffsetDateTime>,
}

impl SecretResolver {
    pub fn new(settings: SecretResolverSettings) -> Result<Self, AppError> {
        validate_optional_base_url(settings.vault_base_url.as_deref(), "vault")?;
        validate_optional_base_url(settings.kms_base_url.as_deref(), "kms")?;
        validate_optional_base_url(
            settings.rotation_gateway_base_url.as_deref(),
            "rotation gateway",
        )?;
        let http = Client::builder()
            .timeout(StdDuration::from_millis(settings.timeout_milliseconds()))
            .build()
            .map_err(|error| {
                AppError::InvalidInput(format!("failed to build secret resolver client: {error}"))
            })?;
        Ok(Self { http, settings })
    }

    pub fn env_only_for_tests() -> Self {
        Self::new(SecretResolverSettings {
            timeout_milliseconds: 1_000,
            vault_enabled: false,
            vault_base_url: None,
            vault_token_ref: None,
            vault_namespace: None,
            kms_enabled: false,
            kms_base_url: None,
            kms_auth_token_ref: None,
            rotation_gateway_enabled: false,
            rotation_gateway_base_url: None,
            rotation_gateway_auth_token_ref: None,
        })
        .expect("env-only secret resolver")
    }

    pub async fn resolve(&self, secret_ref: &str) -> Result<String, AppError> {
        if secret_ref.starts_with("env://") || secret_ref.starts_with("env:") {
            return resolve_env_secret_ref(secret_ref);
        }
        if secret_ref.starts_with("vault://") {
            return self.resolve_vault(secret_ref).await;
        }
        if secret_ref.starts_with("kms://") {
            return self.resolve_kms(secret_ref).await;
        }
        Err(AppError::InvalidInput(
            "unsupported secret_ref scheme; supported schemes are env://, vault://, and kms://"
                .to_string(),
        ))
    }

    pub fn is_configured(&self, secret_ref: &str) -> bool {
        if let Ok(name) = env_name_from_secret_ref(secret_ref) {
            return std::env::var(name).is_ok();
        }
        if secret_ref.starts_with("vault://") {
            return self.settings.vault_enabled
                && self.settings.vault_base_url.is_some()
                && configured_env_token(self.settings.vault_token_ref.as_deref());
        }
        if secret_ref.starts_with("kms://") {
            return self.settings.kms_enabled && self.settings.kms_base_url.is_some();
        }
        false
    }

    pub fn rotation_gateway_configured(&self) -> bool {
        self.settings.rotation_gateway_enabled
            && self.settings.rotation_gateway_base_url.is_some()
            && self
                .settings
                .rotation_gateway_auth_token_ref
                .as_deref()
                .is_none_or(configured_env_token_ref)
    }

    pub async fn rotate_secret_ref(
        &self,
        attempt_id: Uuid,
        tenant_id: Uuid,
        credential_id: Uuid,
        provider_key: &str,
        current_secret_ref: &str,
    ) -> Result<RotatedSecretRef, AppError> {
        if !self.settings.rotation_gateway_enabled {
            return Err(AppError::InvalidInput(
                "secret rotation gateway is disabled".to_string(),
            ));
        }
        validate_secret_ref(current_secret_ref)?;
        let base_url = required_setting(
            self.settings.rotation_gateway_base_url.as_deref(),
            "rotation_gateway_base_url",
        )?;
        let mut request = self
            .http
            .post(format!("{}/rotate", base_url.trim_end_matches('/')))
            .json(&RotateSecretRefRequest {
                attempt_id,
                tenant_id,
                credential_id,
                provider_key,
                current_secret_ref,
            });
        if let Some(token_ref) = self.settings.rotation_gateway_auth_token_ref.as_deref() {
            request = request.bearer_auth(resolve_env_secret_ref(token_ref)?);
        }
        let response = request.send().await.map_err(|_| {
            AppError::InvalidInput("secret rotation gateway request failed".to_string())
        })?;
        if !response.status().is_success() {
            return Err(AppError::InvalidInput(format!(
                "secret rotation gateway failed with status {}",
                response.status().as_u16()
            )));
        }
        let response: RotateSecretRefResponse = response.json().await.map_err(|_| {
            AppError::InvalidInput("secret rotation gateway response is invalid".to_string())
        })?;
        let secret_ref = response.secret_ref.trim().to_string();
        validate_secret_ref(&secret_ref)?;
        if secret_ref == current_secret_ref {
            return Err(AppError::InvalidInput(
                "secret rotation gateway returned the current secret_ref".to_string(),
            ));
        }
        Ok(RotatedSecretRef {
            secret_ref,
            secret_hash: response.secret_hash,
            expires_at: response.expires_at,
        })
    }

    async fn resolve_vault(&self, secret_ref: &str) -> Result<String, AppError> {
        if !self.settings.vault_enabled {
            return Err(AppError::InvalidInput(
                "Vault secret resolver is disabled".to_string(),
            ));
        }
        let base_url = required_setting(self.settings.vault_base_url.as_deref(), "vault_base_url")?;
        let token_ref =
            required_setting(self.settings.vault_token_ref.as_deref(), "vault_token_ref")?;
        let token = resolve_env_secret_ref(token_ref)?;
        let (path, field) = parse_vault_ref(secret_ref)?;
        let mut request = self
            .http
            .get(format!("{}/v1/{path}", base_url.trim_end_matches('/')))
            .header("X-Vault-Token", token);
        if let Some(namespace) = self
            .settings
            .vault_namespace
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            request = request.header("X-Vault-Namespace", namespace);
        }
        let response = request.send().await.map_err(|_| {
            AppError::InvalidInput("Vault secret resolution request failed".to_string())
        })?;
        if !response.status().is_success() {
            return Err(AppError::InvalidInput(format!(
                "Vault secret resolution failed with status {}",
                response.status().as_u16()
            )));
        }
        let value: Value = response.json().await.map_err(|_| {
            AppError::InvalidInput("Vault secret response is invalid JSON".to_string())
        })?;
        value
            .pointer(&format!("/data/data/{field}"))
            .or_else(|| value.pointer(&format!("/data/{field}")))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .ok_or_else(|| {
                AppError::InvalidInput(
                    "Vault secret response does not contain the field".to_string(),
                )
            })
    }

    async fn resolve_kms(&self, secret_ref: &str) -> Result<String, AppError> {
        if !self.settings.kms_enabled {
            return Err(AppError::InvalidInput(
                "KMS secret resolver is disabled".to_string(),
            ));
        }
        let base_url = required_setting(self.settings.kms_base_url.as_deref(), "kms_base_url")?;
        let (key_id, ciphertext) = parse_kms_ref(secret_ref)?;
        let mut request = self
            .http
            .post(format!("{}/decrypt", base_url.trim_end_matches('/')))
            .json(&json!({"key_id": key_id, "ciphertext": ciphertext}));
        if let Some(token_ref) = self.settings.kms_auth_token_ref.as_deref() {
            request = request.bearer_auth(resolve_env_secret_ref(token_ref)?);
        }
        let response = request
            .send()
            .await
            .map_err(|_| AppError::InvalidInput("KMS decrypt request failed".to_string()))?;
        if !response.status().is_success() {
            return Err(AppError::InvalidInput(format!(
                "KMS decrypt failed with status {}",
                response.status().as_u16()
            )));
        }
        let value: Value = response.json().await.map_err(|_| {
            AppError::InvalidInput("KMS decrypt response is invalid JSON".to_string())
        })?;
        let plaintext = value
            .get("plaintext_base64")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                AppError::InvalidInput(
                    "KMS decrypt response must contain plaintext_base64".to_string(),
                )
            })?;
        let decoded = STANDARD
            .decode(plaintext)
            .map_err(|_| AppError::InvalidInput("KMS plaintext is not valid base64".to_string()))?;
        String::from_utf8(decoded)
            .ok()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| AppError::InvalidInput("KMS plaintext is not valid UTF-8".to_string()))
    }
}

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
    let resolver_scheme = secret_ref_scheme(&secret.secret_ref);
    let secret_value = resolve_secret_for_tenant(state, tenant_id, &secret.secret_ref).await?;
    let runtime_credential_id =
        issue_runtime_credential(state, tenant_id, run_id, secret, secret_value).await?;
    let credential_resource_id = credential_id.to_string();
    let mut tx = state.connect_pool.begin().await?;
    if let Err(error) = insert_audit_log_tx(
        &mut tx,
        NewAuditLog {
            tenant_id,
            actor_user_id: None,
            actor_device_id: None,
            session_id: None,
            resource_type: "llm_credential",
            resource_id: &credential_resource_id,
            action: "resolve_runtime",
            decision: "allow",
            policy_version: "secret-resolver-v1",
            reason_code: Some(resolver_scheme),
            run_id: Some(run_id),
            conversation_id: None,
            workflow_run_id: None,
            tool_call_id: None,
            approval_id: None,
            args_hash: None,
            input_summary: Some(resolver_scheme),
            output_summary: Some("short-lived runtime credential issued"),
            risk_level: Some("high"),
            ip: None,
            user_agent: None,
            trace_id: None,
        },
    )
    .await
    {
        let _ = revoke_runtime_credentials_for_credential(state, credential_id).await;
        return Err(error);
    }
    if let Err(error) = tx.commit().await {
        let _ = revoke_runtime_credentials_for_credential(state, credential_id).await;
        return Err(AppError::from(error));
    }
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
          AND c.rotation_started_at IS NULL
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
    let _: i64 = redis::cmd("SADD")
        .arg(runtime_credential_index_key(credential.credential_id))
        .arg(runtime_credential_key(&runtime_credential_id))
        .query_async(&mut conn)
        .await?;
    let _: bool = redis::cmd("EXPIRE")
        .arg(runtime_credential_index_key(credential.credential_id))
        .arg(RUNTIME_CREDENTIAL_TTL_SECONDS)
        .query_async(&mut conn)
        .await?;
    Ok(runtime_credential_id)
}

pub async fn revoke_runtime_credentials_for_credential(
    state: &AppState,
    credential_id: Uuid,
) -> Result<u64, AppError> {
    let mut conn = state
        .redis_client
        .get_multiplexed_async_connection()
        .await?;
    let index_key = runtime_credential_index_key(credential_id);
    let keys: Vec<String> = redis::cmd("SMEMBERS")
        .arg(&index_key)
        .query_async(&mut conn)
        .await?;
    if keys.is_empty() {
        let _: i64 = redis::cmd("DEL")
            .arg(index_key)
            .query_async(&mut conn)
            .await?;
        return Ok(0);
    }
    let mut command = redis::cmd("DEL");
    for key in &keys {
        command.arg(key);
    }
    command.arg(index_key);
    let _: i64 = command.query_async(&mut conn).await?;
    Ok(keys.len() as u64)
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

pub fn resolve_env_secret_ref(secret_ref: &str) -> Result<String, AppError> {
    let env_name = env_name_from_secret_ref(secret_ref)?;
    std::env::var(env_name).map_err(|_| {
        AppError::InvalidInput("secret_ref points to a missing environment variable".to_string())
    })
}

pub async fn resolve_secret_for_tenant(
    state: &AppState,
    tenant_id: Uuid,
    secret_ref: &str,
) -> Result<String, AppError> {
    let Some(id) = secret_ref.strip_prefix("local://") else {
        return state.secret_resolver.resolve(secret_ref).await;
    };
    let secret_id = Uuid::parse_str(id)
        .map_err(|_| AppError::InvalidInput("local secret_ref is invalid".to_string()))?;
    sqlx::query_scalar::<_, String>(
        r#"
        SELECT pgp_sym_decrypt(ciphertext, $3)
        FROM llm_local_secrets
        WHERE id = $1 AND tenant_id = $2
        "#,
    )
    .bind(secret_id)
    .bind(tenant_id)
    .bind(local_secret_encryption_key(&state.internal_shared_token))
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::InvalidInput("local LLM secret is unavailable".to_string()))
}

pub(crate) fn local_secret_encryption_key(internal_shared_token: &str) -> String {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(b"biwork-local-secret-v1\0");
    hasher.update(internal_shared_token.as_bytes());
    hex::encode(hasher.finalize())
}

pub fn validate_secret_ref(secret_ref: &str) -> Result<(), AppError> {
    if let Some(id) = secret_ref.strip_prefix("local://") {
        Uuid::parse_str(id)
            .map_err(|_| AppError::InvalidInput("local secret_ref is invalid".to_string()))?;
        return Ok(());
    }
    if secret_ref.starts_with("env://") || secret_ref.starts_with("env:") {
        env_name_from_secret_ref(secret_ref)?;
        return Ok(());
    }
    if secret_ref.starts_with("vault://") {
        parse_vault_ref(secret_ref)?;
        return Ok(());
    }
    if secret_ref.starts_with("kms://") {
        parse_kms_ref(secret_ref)?;
        return Ok(());
    }
    Err(AppError::InvalidInput(
        "unsupported secret_ref scheme; supported schemes are local://, env://, vault://, and kms://"
            .to_string(),
    ))
}

fn configured_env_token(token_ref: Option<&str>) -> bool {
    token_ref
        .and_then(|value| env_name_from_secret_ref(value).ok())
        .and_then(|name| std::env::var(name).ok())
        .is_some()
}

fn configured_env_token_ref(token_ref: &str) -> bool {
    configured_env_token(Some(token_ref))
}

fn required_setting<'a>(value: Option<&'a str>, field: &str) -> Result<&'a str, AppError> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppError::InvalidInput(format!("secret resolver {field} is required")))
}

fn validate_optional_base_url(value: Option<&str>, resolver: &str) -> Result<(), AppError> {
    let Some(value) = value else {
        return Ok(());
    };
    let parsed = reqwest::Url::parse(value)
        .map_err(|_| AppError::InvalidInput(format!("{resolver} resolver base URL is invalid")))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err(AppError::InvalidInput(format!(
            "{resolver} resolver base URL must use http or https"
        )));
    }
    Ok(())
}

fn parse_vault_ref(secret_ref: &str) -> Result<(&str, &str), AppError> {
    let target = secret_ref
        .strip_prefix("vault://")
        .ok_or_else(|| AppError::InvalidInput("Vault secret_ref must use vault://".to_string()))?;
    let (path, field) = target.split_once('#').ok_or_else(|| {
        AppError::InvalidInput("Vault secret_ref must include #field".to_string())
    })?;
    if !valid_secret_path(path) || !valid_secret_component(field, 128) {
        return Err(AppError::InvalidInput(
            "Vault secret_ref path or field is invalid".to_string(),
        ));
    }
    Ok((path, field))
}

fn parse_kms_ref(secret_ref: &str) -> Result<(&str, &str), AppError> {
    let target = secret_ref
        .strip_prefix("kms://")
        .ok_or_else(|| AppError::InvalidInput("KMS secret_ref must use kms://".to_string()))?;
    let (key_id, ciphertext) = target.split_once('#').ok_or_else(|| {
        AppError::InvalidInput("KMS secret_ref must include #ciphertext".to_string())
    })?;
    let valid_ciphertext = !ciphertext.is_empty()
        && ciphertext.len() <= 32_768
        && ciphertext.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'=' | b'-' | b'_')
        });
    if !valid_secret_component(key_id, 512) || !valid_ciphertext {
        return Err(AppError::InvalidInput(
            "KMS secret_ref key id or ciphertext is invalid".to_string(),
        ));
    }
    Ok((key_id, ciphertext))
}

fn valid_secret_path(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 1024
        && !value.split('/').any(|part| part.is_empty() || part == "..")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'_' | b'-' | b'.'))
}

fn valid_secret_component(value: &str, max_len: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_len
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':' | b'/')
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

fn runtime_credential_index_key(credential_id: Uuid) -> String {
    format!("bibi:runtime-credential-index:{credential_id}")
}

fn secret_ref_scheme(secret_ref: &str) -> &'static str {
    if secret_ref.starts_with("local://") {
        "local"
    } else if secret_ref.starts_with("vault://") {
        "vault"
    } else if secret_ref.starts_with("kms://") {
        "kms"
    } else {
        "env"
    }
}

#[cfg(test)]
mod tests {
    use axum::{
        Json, Router,
        extract::Path,
        http::HeaderMap,
        routing::{get, post},
    };
    use serde_json::json;
    use tokio::net::TcpListener;

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
        assert!(env_name_from_secret_ref("vault://secret/data/tenant/key#token").is_err());
        assert!(env_name_from_secret_ref("env://TENANT/KEY").is_err());
        assert_eq!(
            parse_vault_ref("vault://secret/data/tenant/key#api_key").unwrap(),
            ("secret/data/tenant/key", "api_key")
        );
        assert!(parse_vault_ref("vault://secret/../key#api_key").is_err());
        assert!(parse_kms_ref("kms://alias/tenant-key#Y2lwaGVydGV4dA==").is_ok());
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

    #[tokio::test]
    async fn resolves_vault_kv_v2_and_kms_gateway_without_exposing_ciphertext()
    -> Result<(), Box<dyn std::error::Error>> {
        unsafe {
            std::env::set_var("BIBI_TEST_VAULT_TOKEN", "vault-test-token");
            std::env::set_var("BIBI_TEST_KMS_TOKEN", "kms-test-token");
        }

        async fn vault_secret(Path(path): Path<String>, headers: HeaderMap) -> Json<Value> {
            assert_eq!(path, "secret/data/tenant/provider");
            assert_eq!(
                headers
                    .get("x-vault-token")
                    .and_then(|value| value.to_str().ok()),
                Some("vault-test-token")
            );
            assert_eq!(
                headers
                    .get("x-vault-namespace")
                    .and_then(|value| value.to_str().ok()),
                Some("bibi-work")
            );
            Json(json!({"data": {"data": {"api_key": "vault-secret-value"}}}))
        }

        async fn kms_decrypt(headers: HeaderMap, Json(payload): Json<Value>) -> Json<Value> {
            assert_eq!(
                headers
                    .get("authorization")
                    .and_then(|value| value.to_str().ok()),
                Some("Bearer kms-test-token")
            );
            assert_eq!(payload["key_id"], "alias/tenant-key");
            assert_eq!(payload["ciphertext"], "Y2lwaGVydGV4dA==");
            Json(json!({"plaintext_base64": STANDARD.encode("kms-secret-value")}))
        }

        let router = Router::new()
            .route("/v1/{*path}", get(vault_secret))
            .route("/decrypt", post(kms_decrypt));
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let base_url = format!("http://{}", listener.local_addr()?);
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        let resolver = SecretResolver::new(SecretResolverSettings {
            timeout_milliseconds: 2_000,
            vault_enabled: true,
            vault_base_url: Some(base_url.clone()),
            vault_token_ref: Some("env://BIBI_TEST_VAULT_TOKEN".to_string()),
            vault_namespace: Some("bibi-work".to_string()),
            kms_enabled: true,
            kms_base_url: Some(base_url),
            kms_auth_token_ref: Some("env://BIBI_TEST_KMS_TOKEN".to_string()),
            rotation_gateway_enabled: false,
            rotation_gateway_base_url: None,
            rotation_gateway_auth_token_ref: None,
        })?;

        assert_eq!(
            resolver
                .resolve("vault://secret/data/tenant/provider#api_key")
                .await?,
            "vault-secret-value"
        );
        assert_eq!(
            resolver
                .resolve("kms://alias/tenant-key#Y2lwaGVydGV4dA==")
                .await?,
            "kms-secret-value"
        );
        server.abort();
        Ok(())
    }

    #[tokio::test]
    async fn rotates_secret_refs_through_idempotent_gateway_contract()
    -> Result<(), Box<dyn std::error::Error>> {
        unsafe {
            std::env::set_var("BIBI_TEST_ROTATION_TOKEN", "rotation-control-token");
        }
        async fn rotate(headers: HeaderMap, Json(payload): Json<Value>) -> Json<Value> {
            assert_eq!(
                headers
                    .get("authorization")
                    .and_then(|value| value.to_str().ok()),
                Some("Bearer rotation-control-token")
            );
            assert!(payload["attempt_id"].as_str().is_some());
            assert_eq!(payload["provider_key"], "openai-compatible");
            assert_eq!(payload["current_secret_ref"], "env://OLD_KEY");
            Json(json!({
                "secret_ref": "vault://secret/data/tenant/provider#api_key_next",
                "secret_hash": "sha256:new-secret",
                "expires_at": "2027-01-01T00:00:00Z"
            }))
        }
        let router = Router::new().route("/rotate", post(rotate));
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let base_url = format!("http://{}", listener.local_addr()?);
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
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
            rotation_gateway_auth_token_ref: Some("env://BIBI_TEST_ROTATION_TOKEN".to_string()),
        })?;
        let rotated = resolver
            .rotate_secret_ref(
                Uuid::new_v4(),
                Uuid::new_v4(),
                Uuid::new_v4(),
                "openai-compatible",
                "env://OLD_KEY",
            )
            .await?;
        assert_eq!(
            rotated.secret_ref,
            "vault://secret/data/tenant/provider#api_key_next"
        );
        assert_eq!(rotated.secret_hash.as_deref(), Some("sha256:new-secret"));
        assert!(rotated.expires_at.is_some());
        server.abort();
        Ok(())
    }
}
