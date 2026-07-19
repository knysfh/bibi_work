use axum::{Extension, Json, extract::State, http::StatusCode, response::IntoResponse};
use reqwest::Url;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    features::{
        agent_platform::ferriskey_oidc::{
            AuthorizationCodeTokenExchange, PlatformRequestContext, RefreshTokenExchange,
        },
        core::errors::AppError,
    },
    startup::AppState,
};

use super::biwork_compat_service::{biwork_failure, ok, response_trace_id};

pub(super) const BIWORK_OIDC_CLIENT_ID: &str = "bibi-work-desktop";
pub(super) const BIWORK_DESKTOP_OIDC_REDIRECT_URI: &str = "http://127.0.0.1:48123/callback";
pub(super) const BIWORK_WEB_OIDC_REDIRECT_URI: &str = "http://127.0.0.1:25808/auth/callback";

#[derive(Debug, Deserialize)]
pub struct BiWorkOidcTokenExchangePayload {
    client_id: Option<String>,
    code: Option<String>,
    code_verifier: Option<String>,
    grant_type: Option<String>,
    redirect_uri: Option<String>,
    refresh_token: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct BiWorkOidcTokenRevokePayload {
    client_id: Option<String>,
    refresh_token: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct WebuiUsernamePayload {
    new_username: String,
}

#[derive(Debug)]
enum ValidatedBiWorkOidcTokenExchange {
    AuthorizationCode {
        client_id: String,
        code: String,
        code_verifier: String,
        redirect_uri: String,
    },
    RefreshToken {
        client_id: String,
        refresh_token: String,
    },
}

fn required_trimmed(value: Option<&str>, label: &str) -> Result<String, AppError> {
    let normalized = value.unwrap_or_default().trim();
    if normalized.is_empty() {
        return Err(AppError::InvalidInput(format!("{label} is required")));
    }
    Ok(normalized.to_string())
}

fn is_allowed_biwork_oidc_redirect_uri(value: &str) -> bool {
    if value == BIWORK_DESKTOP_OIDC_REDIRECT_URI {
        return true;
    }

    let Ok(url) = Url::parse(value) else {
        return false;
    };
    if url.scheme() != "http"
        || url.path() != "/auth/callback"
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_none()
    {
        return false;
    }
    matches!(url.host_str(), Some("127.0.0.1" | "localhost"))
}

fn validate_biwork_oidc_token_exchange(
    payload: &BiWorkOidcTokenExchangePayload,
) -> Result<ValidatedBiWorkOidcTokenExchange, AppError> {
    let client_id = required_trimmed(payload.client_id.as_deref(), "client_id")?;
    if client_id != BIWORK_OIDC_CLIENT_ID {
        return Err(AppError::InvalidInput(
            "client_id is not allowed".to_string(),
        ));
    }

    let grant_type = payload
        .grant_type
        .as_deref()
        .map(str::trim)
        .unwrap_or("authorization_code");
    if grant_type.eq_ignore_ascii_case("refresh_token") {
        return Ok(ValidatedBiWorkOidcTokenExchange::RefreshToken {
            client_id,
            refresh_token: required_trimmed(payload.refresh_token.as_deref(), "refresh_token")?,
        });
    }
    if !grant_type.eq_ignore_ascii_case("authorization_code") {
        return Err(AppError::InvalidInput(
            "grant_type must be authorization_code or refresh_token".to_string(),
        ));
    }
    let redirect_uri = required_trimmed(payload.redirect_uri.as_deref(), "redirect_uri")?;
    if !is_allowed_biwork_oidc_redirect_uri(&redirect_uri) {
        return Err(AppError::InvalidInput(
            "redirect_uri is not allowed".to_string(),
        ));
    }
    Ok(ValidatedBiWorkOidcTokenExchange::AuthorizationCode {
        client_id,
        code: required_trimmed(payload.code.as_deref(), "code")?,
        code_verifier: required_trimmed(payload.code_verifier.as_deref(), "code_verifier")?,
        redirect_uri,
    })
}

pub async fn biwork_get_oidc_config(
    State(state): State<AppState>,
) -> Result<Json<Value>, AppError> {
    let config = serde_json::to_value(state.ferriskey_oidc.public_config().await?)
        .map_err(|_| AppError::InvalidInput("failed to encode OIDC config".to_string()))?;
    Ok(ok(json!({
        "issuer": config.get("issuer").cloned().unwrap_or(Value::Null),
        "client_id": BIWORK_OIDC_CLIENT_ID,
        "authorization_endpoint": config.get("authorization_endpoint").cloned().unwrap_or(Value::Null),
        "token_endpoint": config.get("token_endpoint").cloned().unwrap_or(Value::Null),
        "revocation_endpoint": config.get("revocation_endpoint").cloned().unwrap_or(Value::Null),
        "token_exchange_endpoint": "/api/auth/oidc/token",
        "jwks_uri": config.get("jwks_uri").cloned().unwrap_or(Value::Null),
        "scopes": ["openid", "profile", "email", "roles"],
        "desktop_callback": {
            "kind": "loopback",
            "redirect_uri": BIWORK_DESKTOP_OIDC_REDIRECT_URI,
        },
        "web_callback": {
            "redirect_uri": BIWORK_WEB_OIDC_REDIRECT_URI,
        },
        "backend_audience": config.get("audience").cloned().unwrap_or(Value::Null),
    })))
}

pub async fn biwork_exchange_oidc_token(
    State(state): State<AppState>,
    Json(payload): Json<BiWorkOidcTokenExchangePayload>,
) -> Result<Json<Value>, AppError> {
    let exchange = validate_biwork_oidc_token_exchange(&payload)?;
    let token_response = match exchange {
        ValidatedBiWorkOidcTokenExchange::AuthorizationCode {
            client_id,
            code,
            code_verifier,
            redirect_uri,
        } => {
            state
                .ferriskey_oidc
                .exchange_authorization_code(AuthorizationCodeTokenExchange {
                    client_id: &client_id,
                    code: &code,
                    code_verifier: &code_verifier,
                    redirect_uri: &redirect_uri,
                })
                .await?
        }
        ValidatedBiWorkOidcTokenExchange::RefreshToken {
            client_id,
            refresh_token,
        } => {
            state
                .ferriskey_oidc
                .exchange_refresh_token(RefreshTokenExchange {
                    client_id: &client_id,
                    refresh_token: &refresh_token,
                })
                .await?
        }
    };
    Ok(ok(token_response))
}

pub async fn biwork_revoke_oidc_token(
    State(state): State<AppState>,
    Json(payload): Json<BiWorkOidcTokenRevokePayload>,
) -> Result<Json<Value>, AppError> {
    let client_id = required_trimmed(payload.client_id.as_deref(), "client_id")?;
    if client_id != BIWORK_OIDC_CLIENT_ID {
        return Err(AppError::InvalidInput(
            "client_id is not allowed".to_string(),
        ));
    }
    let refresh_token = required_trimmed(payload.refresh_token.as_deref(), "refresh_token")?;
    state
        .ferriskey_oidc
        .revoke_refresh_token(&client_id, &refresh_token)
        .await?;
    Ok(ok(json!({ "revoked": true })))
}

pub async fn biwork_auth_status(State(state): State<AppState>) -> Result<Json<Value>, AppError> {
    let user_count = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT COUNT(*)::bigint
        FROM platform_users
        WHERE status = 'active'
        "#,
    )
    .fetch_one(&state.connect_pool)
    .await?;

    Ok(Json(json!({
        "success": true,
        "trace_id": response_trace_id(),
        "needs_setup": false,
        "user_count": user_count,
        "is_authenticated": false,
        "auth_mode": "ferriskey_oidc",
        "data": {
            "needs_setup": false,
            "user_count": user_count,
            "is_authenticated": false,
            "auth_mode": "ferriskey_oidc",
        }
    })))
}

pub async fn biwork_get_system_user() -> Result<Json<Value>, AppError> {
    Ok(ok(json!({
        "id": "system_default_user",
        "username": "admin",
        "auth_mode": "ferriskey_oidc",
        "password_configured": false,
    })))
}

pub async fn biwork_seed_system_user_credentials() -> impl IntoResponse {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(biwork_failure(
            "PASSWORD_AUTH_UNSUPPORTED",
            "local WebUI password credentials are not available; use FerrisKey/OIDC",
            json!({ "auth_mode": "ferriskey_oidc" }),
        )),
    )
}

pub async fn biwork_auth_user(
    Extension(ctx): Extension<PlatformRequestContext>,
) -> Result<Json<Value>, AppError> {
    Ok(Json(json!({
        "success": true,
        "trace_id": response_trace_id(),
        "user": {
            "id": ctx.platform_user_id.to_string(),
            "username": ctx.preferred_username.unwrap_or(ctx.ferriskey_subject),
        }
    })))
}

pub async fn biwork_logout(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
) -> Result<Json<Value>, AppError> {
    sqlx::query(
        r#"
        UPDATE platform_sessions
        SET revoked_at = CURRENT_TIMESTAMP,
            revocation_reason = 'user_logout',
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1 AND tenant_id = $2 AND user_id = $3
        "#,
    )
    .bind(ctx.session_id)
    .bind(ctx.tenant_id)
    .bind(ctx.platform_user_id)
    .execute(&state.connect_pool)
    .await?;

    Ok(ok(json!({ "revoked": true })))
}

pub async fn biwork_webui_password_auth_unsupported() -> impl IntoResponse {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(biwork_failure(
            "PASSWORD_AUTH_UNSUPPORTED",
            "local WebUI password authentication is not available; use FerrisKey/OIDC",
            json!({}),
        )),
    )
}

pub async fn biwork_generate_webui_qr_token() -> impl IntoResponse {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(biwork_failure(
            "WEBUI_QR_LOGIN_UNSUPPORTED",
            "WebUI QR login tokens are not implemented for the FerrisKey/OIDC backend",
            json!({}),
        )),
    )
}

pub async fn biwork_change_webui_username(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<WebuiUsernamePayload>,
) -> Result<Json<Value>, AppError> {
    let username = payload.new_username.trim();
    if username.is_empty() {
        return Err(AppError::InvalidInput(
            "new_username is required".to_string(),
        ));
    }

    sqlx::query(
        r#"
        UPDATE platform_users
        SET username = $1,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $2 AND tenant_id = $3
        "#,
    )
    .bind(username)
    .bind(ctx.platform_user_id)
    .bind(ctx.tenant_id)
    .execute(&state.connect_pool)
    .await?;

    Ok(ok(json!({ "username": username })))
}

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;

    use super::*;

    fn oidc_payload(redirect_uri: &str) -> BiWorkOidcTokenExchangePayload {
        BiWorkOidcTokenExchangePayload {
            client_id: Some(BIWORK_OIDC_CLIENT_ID.to_string()),
            code: Some("code-1".to_string()),
            code_verifier: Some("verifier-1".to_string()),
            grant_type: Some("authorization_code".to_string()),
            redirect_uri: Some(redirect_uri.to_string()),
            refresh_token: None,
        }
    }

    fn refresh_payload() -> BiWorkOidcTokenExchangePayload {
        BiWorkOidcTokenExchangePayload {
            client_id: Some(BIWORK_OIDC_CLIENT_ID.to_string()),
            code: None,
            code_verifier: None,
            grant_type: Some("refresh_token".to_string()),
            redirect_uri: None,
            refresh_token: Some("refresh-1".to_string()),
        }
    }

    #[test]
    fn oidc_token_exchange_validation_allows_loopback_redirects() {
        assert!(
            validate_biwork_oidc_token_exchange(&oidc_payload(BIWORK_DESKTOP_OIDC_REDIRECT_URI))
                .is_ok()
        );
        assert!(
            validate_biwork_oidc_token_exchange(&oidc_payload(
                "http://127.0.0.1:25809/auth/callback"
            ))
            .is_ok()
        );
        assert!(
            validate_biwork_oidc_token_exchange(&oidc_payload(
                "http://localhost:25810/auth/callback"
            ))
            .is_ok()
        );
    }

    #[test]
    fn oidc_token_exchange_validation_rejects_unowned_client_or_redirect() {
        let mut bad_client = oidc_payload(BIWORK_WEB_OIDC_REDIRECT_URI);
        bad_client.client_id = Some("other-client".to_string());
        assert!(matches!(
            validate_biwork_oidc_token_exchange(&bad_client),
            Err(AppError::InvalidInput(_))
        ));
        assert!(matches!(
            validate_biwork_oidc_token_exchange(&oidc_payload("https://example.com/auth/callback")),
            Err(AppError::InvalidInput(_))
        ));
        assert!(matches!(
            validate_biwork_oidc_token_exchange(&oidc_payload("http://127.0.0.1:25808/other")),
            Err(AppError::InvalidInput(_))
        ));
    }

    #[test]
    fn oidc_token_exchange_validation_accepts_refresh_grant_without_pkce_fields() {
        assert!(matches!(
            validate_biwork_oidc_token_exchange(&refresh_payload()),
            Ok(ValidatedBiWorkOidcTokenExchange::RefreshToken { .. })
        ));
    }

    #[tokio::test]
    async fn webui_password_compat_routes_fail_visibly_under_oidc() {
        let seed_response = biwork_seed_system_user_credentials().await.into_response();
        assert_eq!(seed_response.status(), StatusCode::NOT_IMPLEMENTED);
        let seed_body = to_bytes(seed_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let seed_payload: Value = serde_json::from_slice(&seed_body).unwrap();
        assert_eq!(seed_payload["success"], false);
        assert_eq!(seed_payload["code"], "PASSWORD_AUTH_UNSUPPORTED");
        assert_eq!(seed_payload["details"]["auth_mode"], "ferriskey_oidc");

        let password_response = biwork_webui_password_auth_unsupported()
            .await
            .into_response();
        assert_eq!(password_response.status(), StatusCode::NOT_IMPLEMENTED);
        let password_body = to_bytes(password_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let password_payload: Value = serde_json::from_slice(&password_body).unwrap();
        assert_eq!(password_payload["success"], false);
        assert_eq!(password_payload["code"], "PASSWORD_AUTH_UNSUPPORTED");

        let qr_response = biwork_generate_webui_qr_token().await.into_response();
        assert_eq!(qr_response.status(), StatusCode::NOT_IMPLEMENTED);
        let qr_body = to_bytes(qr_response.into_body(), usize::MAX).await.unwrap();
        let qr_payload: Value = serde_json::from_slice(&qr_body).unwrap();
        assert_eq!(qr_payload["success"], false);
        assert_eq!(qr_payload["code"], "WEBUI_QR_LOGIN_UNSUPPORTED");
    }
}
