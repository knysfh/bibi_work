use std::{collections::HashSet, sync::Arc, time::Instant};

use axum::{
    Json,
    extract::{Request, State},
    http::{
        HeaderMap, HeaderValue,
        header::{AUTHORIZATION, HeaderName},
    },
    middleware::Next,
    response::Response,
};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, jwk::JwkSet};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use time::OffsetDateTime;
use tokio::sync::RwLock;
use tracing::warn;
use uuid::Uuid;

use crate::{
    configuration::FerrisKeySettings,
    features::{agent_platform::process_metrics, core::errors::AppError},
    startup::AppState,
};

const TRACE_ID_HEADER: &str = "x-trace-id";

#[derive(Clone)]
pub struct FerrisKeyOidcVerifier {
    http: Client,
    issuer: String,
    audience: String,
    trusted_authorized_parties: Vec<String>,
    discovery_url: String,
    configured_jwks_uri: Option<String>,
    default_tenant_slug: String,
    cached_jwks_uri: Arc<RwLock<Option<String>>>,
    cached_jwks: Arc<RwLock<Option<JwkSet>>>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct PlatformRequestContext {
    pub tenant_id: Uuid,
    pub platform_user_id: Uuid,
    pub ferriskey_subject: String,
    pub preferred_username: Option<String>,
    pub email: Option<String>,
    pub roles: Vec<String>,
    pub session_id: Uuid,
    pub device_id: Uuid,
    pub trace_id: String,
    pub token_jti: Option<String>,
    pub token_exp: OffsetDateTime,
}

#[derive(Debug, Deserialize)]
struct OidcDiscoveryDocument {
    issuer: String,
    jwks_uri: String,
    authorization_endpoint: Option<String>,
    token_endpoint: Option<String>,
    revocation_endpoint: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct OidcPublicConfig {
    issuer: String,
    audience: String,
    authorization_endpoint: Option<String>,
    token_endpoint: Option<String>,
    revocation_endpoint: Option<String>,
    jwks_uri: String,
}

#[derive(Debug)]
pub struct AuthorizationCodeTokenExchange<'a> {
    pub client_id: &'a str,
    pub code: &'a str,
    pub code_verifier: &'a str,
    pub redirect_uri: &'a str,
}

#[derive(Debug)]
pub struct RefreshTokenExchange<'a> {
    pub client_id: &'a str,
    pub refresh_token: &'a str,
}

#[derive(Debug, Deserialize, Clone)]
pub struct FerrisKeyTokenClaims {
    pub iss: String,
    pub sub: String,
    pub aud: Option<Value>,
    pub azp: Option<String>,
    pub exp: i64,
    pub nbf: Option<i64>,
    pub iat: Option<i64>,
    pub jti: Option<String>,
    pub sid: Option<String>,
    pub session_state: Option<String>,
    pub preferred_username: Option<String>,
    pub email: Option<String>,
    pub name: Option<String>,
    #[serde(default)]
    pub roles: Vec<String>,
    pub realm_access: Option<RealmAccessClaim>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RealmAccessClaim {
    #[serde(default)]
    pub roles: Vec<String>,
}

impl FerrisKeyTokenClaims {
    fn effective_roles(&self) -> Vec<String> {
        let mut roles = HashSet::new();
        for role in &self.roles {
            let role = role.trim();
            if !role.is_empty() {
                roles.insert(role.to_string());
            }
        }
        if let Some(realm_access) = &self.realm_access {
            for role in &realm_access.roles {
                let role = role.trim();
                if !role.is_empty() {
                    roles.insert(role.to_string());
                }
            }
        }
        let mut roles = roles.into_iter().collect::<Vec<_>>();
        roles.sort();
        roles
    }

    fn session_key(&self) -> Option<String> {
        self.sid
            .clone()
            .or_else(|| self.session_state.clone())
            .or_else(|| self.jti.clone())
    }
}

impl FerrisKeyOidcVerifier {
    pub fn new(settings: FerrisKeySettings) -> Result<Self, reqwest::Error> {
        let http = Client::builder()
            .timeout(std::time::Duration::from_millis(
                settings.timeout_milliseconds,
            ))
            .build()?;
        let trusted_authorized_parties = normalize_trusted_authorized_parties(
            &settings.audience,
            settings.trusted_authorized_parties,
        );

        Ok(Self {
            http,
            issuer: settings.issuer,
            audience: settings.audience,
            trusted_authorized_parties,
            discovery_url: settings.discovery_url,
            configured_jwks_uri: settings.jwks_uri,
            default_tenant_slug: settings.default_tenant_slug,
            cached_jwks_uri: Arc::new(RwLock::new(None)),
            cached_jwks: Arc::new(RwLock::new(None)),
        })
    }

    pub async fn public_config(&self) -> Result<OidcPublicConfig, AppError> {
        let discovery = self.discovery_document().await?;
        Ok(OidcPublicConfig {
            issuer: discovery.issuer,
            audience: self.audience.clone(),
            authorization_endpoint: discovery.authorization_endpoint,
            token_endpoint: discovery.token_endpoint,
            revocation_endpoint: discovery.revocation_endpoint,
            jwks_uri: discovery.jwks_uri,
        })
    }

    pub async fn exchange_authorization_code(
        &self,
        exchange: AuthorizationCodeTokenExchange<'_>,
    ) -> Result<Value, AppError> {
        let discovery = self.discovery_document().await?;
        let token_endpoint = discovery.token_endpoint.ok_or_else(|| {
            AppError::Unauthorized("FerrisKey token endpoint is not configured".to_string())
        })?;

        let form_body = form_urlencoded(&[
            ("grant_type", "authorization_code"),
            ("client_id", exchange.client_id),
            ("code", exchange.code),
            ("code_verifier", exchange.code_verifier),
            ("redirect_uri", exchange.redirect_uri),
        ]);

        let response = self
            .http
            .post(token_endpoint)
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .body(form_body)
            .send()
            .await
            .map_err(|_| {
                AppError::Unauthorized(
                    "failed to exchange FerrisKey authorization code".to_string(),
                )
            })?
            .error_for_status()
            .map_err(|_| {
                AppError::Unauthorized("FerrisKey authorization code exchange failed".to_string())
            })?;

        response
            .json::<Value>()
            .await
            .map_err(|_| AppError::Unauthorized("invalid FerrisKey token response".to_string()))
    }

    pub async fn exchange_refresh_token(
        &self,
        exchange: RefreshTokenExchange<'_>,
    ) -> Result<Value, AppError> {
        let discovery = self.discovery_document().await?;
        let token_endpoint = discovery.token_endpoint.ok_or_else(|| {
            AppError::UpstreamUnavailable("FerrisKey token endpoint is not configured".to_string())
        })?;
        let form_body = form_urlencoded(&[
            ("grant_type", "refresh_token"),
            ("client_id", exchange.client_id),
            ("refresh_token", exchange.refresh_token),
        ]);

        let response = self
            .http
            .post(token_endpoint)
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .body(form_body)
            .send()
            .await
            .map_err(|_| {
                AppError::UpstreamUnavailable(
                    "failed to contact FerrisKey token endpoint".to_string(),
                )
            })?;

        if matches!(response.status().as_u16(), 400 | 401) {
            return Err(AppError::Unauthorized(
                "FerrisKey refresh token was rejected".to_string(),
            ));
        }
        let response = response.error_for_status().map_err(|_| {
            AppError::UpstreamUnavailable("FerrisKey token refresh failed".to_string())
        })?;
        response.json::<Value>().await.map_err(|_| {
            AppError::UpstreamUnavailable("invalid FerrisKey refresh response".to_string())
        })
    }

    pub async fn revoke_refresh_token(
        &self,
        client_id: &str,
        refresh_token: &str,
    ) -> Result<(), AppError> {
        let discovery = self.discovery_document().await?;
        let Some(revocation_endpoint) = discovery.revocation_endpoint else {
            return Ok(());
        };
        let form_body = form_urlencoded(&[
            ("client_id", client_id),
            ("token", refresh_token),
            ("token_type_hint", "refresh_token"),
        ]);
        self.http
            .post(revocation_endpoint)
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .body(form_body)
            .send()
            .await
            .map_err(|_| {
                AppError::UpstreamUnavailable(
                    "failed to contact FerrisKey revocation endpoint".to_string(),
                )
            })?
            .error_for_status()
            .map_err(|_| {
                AppError::UpstreamUnavailable("FerrisKey token revocation failed".to_string())
            })?;
        Ok(())
    }

    pub async fn authenticate(
        &self,
        pool: &PgPool,
        headers: &HeaderMap,
        token: &str,
        trace_id: String,
    ) -> Result<PlatformRequestContext, AppError> {
        let started_at = Instant::now();
        let result = async {
            let claims = self.verify_access_token(token).await?;
            self.upsert_projection(pool, headers, token, claims, trace_id)
                .await
        }
        .await;
        process_metrics::observe_oidc_auth(started_at.elapsed(), result.is_ok());
        result
    }

    async fn verify_access_token(&self, token: &str) -> Result<FerrisKeyTokenClaims, AppError> {
        let header = decode_header(token)
            .map_err(|_| AppError::Unauthorized("invalid FerrisKey token header".to_string()))?;
        let kid = header
            .kid
            .as_deref()
            .ok_or_else(|| AppError::Unauthorized("FerrisKey token missing kid".to_string()))?;

        let decoding_key = self.decoding_key(kid).await?;
        match self.decode_with_key(token, header.alg, &decoding_key) {
            Ok(claims) => Ok(claims),
            Err(first_error) => {
                warn!("FerrisKey token verification failed; refreshing JWKS: {first_error}");
                self.refresh_jwks().await?;
                let decoding_key = self.decoding_key(kid).await?;
                self.decode_with_key(token, header.alg, &decoding_key)
            }
        }
    }

    fn decode_with_key(
        &self,
        token: &str,
        algorithm: Algorithm,
        decoding_key: &DecodingKey,
    ) -> Result<FerrisKeyTokenClaims, AppError> {
        let mut validation = Validation::new(algorithm);
        validation.set_issuer(&[self.issuer.as_str()]);
        validation.validate_aud = false;
        validation.set_required_spec_claims(&["exp", "iss", "sub"]);
        validation.validate_nbf = true;

        let data = decode::<FerrisKeyTokenClaims>(token, decoding_key, &validation)
            .map_err(|_| AppError::Unauthorized("invalid FerrisKey access token".to_string()))?;
        if data.claims.sub.trim().is_empty() {
            return Err(AppError::Unauthorized(
                "FerrisKey token missing sub".to_string(),
            ));
        }
        if !audience_or_authorized_party_matches(
            &data.claims,
            &self.audience,
            &self.trusted_authorized_parties,
        ) {
            return Err(AppError::Unauthorized(
                "FerrisKey token audience mismatch".to_string(),
            ));
        }
        Ok(data.claims)
    }

    async fn decoding_key(&self, kid: &str) -> Result<DecodingKey, AppError> {
        if let Some(key) = self.cached_decoding_key(kid).await? {
            return Ok(key);
        }
        self.refresh_jwks().await?;
        self.cached_decoding_key(kid)
            .await?
            .ok_or_else(|| AppError::Unauthorized("FerrisKey JWKS has no matching kid".to_string()))
    }

    async fn cached_decoding_key(&self, kid: &str) -> Result<Option<DecodingKey>, AppError> {
        let guard = self.cached_jwks.read().await;
        let Some(jwks) = guard.as_ref() else {
            return Ok(None);
        };
        let Some(jwk) = jwks.find(kid) else {
            return Ok(None);
        };
        let key = DecodingKey::from_jwk(jwk)
            .map_err(|_| AppError::Unauthorized("invalid FerrisKey JWK".to_string()))?;
        Ok(Some(key))
    }

    async fn refresh_jwks(&self) -> Result<(), AppError> {
        let started_at = Instant::now();
        let result = async {
            let jwks_uri = self.jwks_uri().await?;
            let jwks = self
                .http
                .get(&jwks_uri)
                .send()
                .await
                .map_err(|_| AppError::Unauthorized("failed to fetch FerrisKey JWKS".to_string()))?
                .error_for_status()
                .map_err(|_| AppError::Unauthorized("FerrisKey JWKS request failed".to_string()))?
                .json::<JwkSet>()
                .await
                .map_err(|_| AppError::Unauthorized("invalid FerrisKey JWKS".to_string()))?;
            *self.cached_jwks.write().await = Some(jwks);
            Ok(())
        }
        .await;
        process_metrics::observe_jwks_refresh(started_at.elapsed(), result.is_ok());
        result
    }

    async fn jwks_uri(&self) -> Result<String, AppError> {
        if let Some(uri) = self.configured_jwks_uri.as_deref() {
            return Ok(uri.to_string());
        }
        if let Some(uri) = self.cached_jwks_uri.read().await.as_ref() {
            return Ok(uri.clone());
        }
        let discovery = self.discovery_document().await?;
        let jwks_uri = discovery.jwks_uri;
        *self.cached_jwks_uri.write().await = Some(jwks_uri.clone());
        Ok(jwks_uri)
    }

    async fn discovery_document(&self) -> Result<OidcDiscoveryDocument, AppError> {
        let discovery = self
            .http
            .get(&self.discovery_url)
            .send()
            .await
            .map_err(|_| AppError::Unauthorized("failed to fetch FerrisKey discovery".to_string()))?
            .error_for_status()
            .map_err(|_| AppError::Unauthorized("FerrisKey discovery request failed".to_string()))?
            .json::<OidcDiscoveryDocument>()
            .await
            .map_err(|_| AppError::Unauthorized("invalid FerrisKey discovery".to_string()))?;

        if discovery.issuer != self.issuer {
            return Err(AppError::Unauthorized(
                "FerrisKey discovery issuer mismatch".to_string(),
            ));
        }
        Ok(discovery)
    }

    async fn upsert_projection(
        &self,
        pool: &PgPool,
        headers: &HeaderMap,
        token: &str,
        claims: FerrisKeyTokenClaims,
        trace_id: String,
    ) -> Result<PlatformRequestContext, AppError> {
        let tenant_row =
            sqlx::query("SELECT id FROM tenants WHERE slug = $1 AND deleted_at IS NULL")
                .bind(&self.default_tenant_slug)
                .fetch_optional(pool)
                .await?
                .ok_or_else(|| {
                    AppError::Unauthorized(format!(
                        "tenant bootstrap required for slug {}",
                        self.default_tenant_slug
                    ))
                })?;
        let tenant_id: Uuid = tenant_row.try_get("id")?;
        let token_exp = OffsetDateTime::from_unix_timestamp(claims.exp).map_err(|_| {
            AppError::Unauthorized("FerrisKey token exp is out of range".to_string())
        })?;
        let roles = claims.effective_roles();
        let session_key = claims.session_key().ok_or_else(|| {
            AppError::Unauthorized("FerrisKey token missing sid/session_state/jti".to_string())
        })?;
        let username = claims
            .preferred_username
            .clone()
            .unwrap_or_else(|| claims.sub.clone());
        let display_name = claims
            .name
            .clone()
            .or_else(|| claims.preferred_username.clone());
        let user_agent = headers
            .get("user-agent")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("unknown");
        let source_ip = headers
            .get("x-forwarded-for")
            .or_else(|| headers.get("x-real-ip"))
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(',').next())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        // A bearer token may legitimately traverse the Electron renderer, main
        // process, and local API clients with different User-Agent values. Bind
        // the projected device to the stable OIDC session instead of mutable
        // transport metadata so those requests cannot move an active websocket
        // session between device rows.
        let device_fingerprint = oidc_device_fingerprint(&session_key);
        let device_name = format!("oidc:{}", user_agent.chars().take(96).collect::<String>());
        let token_hash = sha256_hex(token.as_bytes());
        let roles_snapshot = serde_json::to_value(&roles)
            .map_err(|_| AppError::InvalidInput("failed to encode roles".to_string()))?;

        let mut tx = pool.begin().await?;

        let user_row = sqlx::query(
            r#"
            INSERT INTO platform_users (
                tenant_id, ferriskey_subject, username, email, display_name, status
            )
            VALUES ($1, $2, $3, $4, $5, 'active')
            ON CONFLICT (tenant_id, ferriskey_subject)
            DO UPDATE SET
                username = COALESCE(EXCLUDED.username, platform_users.username),
                email = COALESCE(EXCLUDED.email, platform_users.email),
                display_name = COALESCE(EXCLUDED.display_name, platform_users.display_name),
                status = 'active',
                updated_at = CURRENT_TIMESTAMP
            RETURNING id
            "#,
        )
        .bind(tenant_id)
        .bind(&claims.sub)
        .bind(username)
        .bind(&claims.email)
        .bind(display_name)
        .fetch_one(&mut *tx)
        .await?;
        let platform_user_id: Uuid = user_row.try_get("id")?;

        if let Some(row) = sqlx::query(
            r#"
            SELECT revoked_at
            FROM devices
            WHERE tenant_id = $1 AND user_id = $2 AND device_fingerprint = $3
            "#,
        )
        .bind(tenant_id)
        .bind(platform_user_id)
        .bind(&device_fingerprint)
        .fetch_optional(&mut *tx)
        .await?
        {
            let revoked_at: Option<OffsetDateTime> = row.try_get("revoked_at")?;
            if revoked_at.is_some() {
                return Err(AppError::Unauthorized(
                    "platform device has been revoked".to_string(),
                ));
            }
        }

        let device_row = sqlx::query(
            r#"
            INSERT INTO devices (
                tenant_id, user_id, device_fingerprint, device_name, platform, trust_level,
                app_kind, last_seen_at
            )
            VALUES ($1, $2, $3, $4, 'oidc', 'standard', 'biwork-desktop', CURRENT_TIMESTAMP)
            ON CONFLICT (tenant_id, user_id, device_fingerprint)
            DO UPDATE SET
                device_name = EXCLUDED.device_name,
                app_kind = EXCLUDED.app_kind,
                last_seen_at = CURRENT_TIMESTAMP,
                updated_at = CURRENT_TIMESTAMP
            RETURNING id
            "#,
        )
        .bind(tenant_id)
        .bind(platform_user_id)
        .bind(&device_fingerprint)
        .bind(device_name)
        .fetch_one(&mut *tx)
        .await?;
        let device_id: Uuid = device_row.try_get("id")?;

        if let Some(row) = sqlx::query(
            r#"
            SELECT revoked_at
            FROM platform_sessions
            WHERE tenant_id = $1
              AND user_id = $2
              AND ferriskey_session_state = $3
            "#,
        )
        .bind(tenant_id)
        .bind(platform_user_id)
        .bind(&session_key)
        .fetch_optional(&mut *tx)
        .await?
        {
            let revoked_at: Option<OffsetDateTime> = row.try_get("revoked_at")?;
            if revoked_at.is_some() {
                return Err(AppError::Unauthorized(
                    "platform session has been revoked".to_string(),
                ));
            }
        }

        let session_row = sqlx::query(
            r#"
            INSERT INTO platform_sessions (
                tenant_id, user_id, device_id, ferriskey_subject, ferriskey_session_state,
                token_jti, token_exp, roles_snapshot, token_hash, last_seen_at,
                source_ip, user_agent, client_kind
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, CURRENT_TIMESTAMP, $10, $11, 'desktop')
            ON CONFLICT (tenant_id, user_id, ferriskey_session_state)
            DO UPDATE SET
                device_id = EXCLUDED.device_id,
                token_jti = EXCLUDED.token_jti,
                token_exp = EXCLUDED.token_exp,
                roles_snapshot = EXCLUDED.roles_snapshot,
                token_hash = EXCLUDED.token_hash,
                last_seen_at = CURRENT_TIMESTAMP,
                source_ip = EXCLUDED.source_ip,
                user_agent = EXCLUDED.user_agent,
                client_kind = EXCLUDED.client_kind,
                updated_at = CURRENT_TIMESTAMP
            RETURNING id
            "#,
        )
        .bind(tenant_id)
        .bind(platform_user_id)
        .bind(device_id)
        .bind(&claims.sub)
        .bind(session_key)
        .bind(&claims.jti)
        .bind(token_exp)
        .bind(roles_snapshot)
        .bind(token_hash)
        .bind(source_ip)
        .bind(user_agent)
        .fetch_one(&mut *tx)
        .await?;
        let session_id: Uuid = session_row.try_get("id")?;

        tx.commit()
            .await
            .map_err(|_| AppError::DatabaseTransaction)?;

        Ok(PlatformRequestContext {
            tenant_id,
            platform_user_id,
            ferriskey_subject: claims.sub,
            preferred_username: claims.preferred_username,
            email: claims.email,
            roles,
            session_id,
            device_id,
            trace_id,
            token_jti: claims.jti,
            token_exp,
        })
    }
}

pub async fn ferriskey_access_token_middleware(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Result<Response, AppError> {
    let trace_id = request_trace_id(request.headers());
    let token = bearer_token(request.headers())?;
    let context = state
        .ferriskey_oidc
        .authenticate(
            &state.connect_pool,
            request.headers(),
            token,
            trace_id.clone(),
        )
        .await?;
    request.extensions_mut().insert(context);
    let mut response = next.run(request).await;
    insert_trace_id_header(response.headers_mut(), &trace_id);
    Ok(response)
}

pub async fn biwork_trace_id_middleware(request: Request, next: Next) -> Response {
    let trace_id = request_trace_id(request.headers());
    let mut response = next.run(request).await;
    insert_trace_id_header(response.headers_mut(), &trace_id);
    response
}

pub async fn get_oidc_config(
    State(state): State<AppState>,
) -> Result<Json<OidcPublicConfig>, AppError> {
    state.ferriskey_oidc.public_config().await.map(Json)
}

fn bearer_token(headers: &HeaderMap) -> Result<&str, AppError> {
    let Some(value) = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
    else {
        return Err(AppError::MissingAuthHeader);
    };
    value
        .strip_prefix("Bearer ")
        .ok_or(AppError::InvalidAuthHeaderFormat)
}

pub(crate) fn request_trace_id(headers: &HeaderMap) -> String {
    headers
        .get(TRACE_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(normalize_trace_id)
        .or_else(|| {
            headers
                .get("traceparent")
                .and_then(|value| value.to_str().ok())
                .and_then(trace_id_from_traceparent)
        })
        .unwrap_or_else(|| Uuid::new_v4().to_string())
}

fn normalize_trace_id(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value.len() > 128 {
        return None;
    }
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | ':' | '/'))
    {
        return Some(value.to_string());
    }
    None
}

fn trace_id_from_traceparent(value: &str) -> Option<String> {
    let trace_id = value.trim().split('-').nth(1)?;
    if trace_id.len() == 32
        && trace_id != "00000000000000000000000000000000"
        && trace_id.chars().all(|ch| ch.is_ascii_hexdigit())
    {
        return Some(trace_id.to_ascii_lowercase());
    }
    None
}

fn insert_trace_id_header(headers: &mut HeaderMap, trace_id: &str) {
    if let Ok(value) = HeaderValue::from_str(trace_id) {
        headers.insert(HeaderName::from_static(TRACE_ID_HEADER), value);
    }
}

fn audience_or_authorized_party_matches(
    claims: &FerrisKeyTokenClaims,
    expected_audience: &str,
    trusted_authorized_parties: &[String],
) -> bool {
    if claims.azp.as_deref() == Some(expected_audience) {
        return true;
    }
    if claims.azp.as_deref().is_some_and(|azp| {
        trusted_authorized_parties
            .iter()
            .any(|trusted| trusted == azp)
    }) {
        return true;
    }

    match claims.aud.as_ref() {
        Some(Value::String(audience)) => audience == expected_audience,
        Some(Value::Array(audiences)) => audiences
            .iter()
            .any(|audience| audience.as_str() == Some(expected_audience)),
        _ => false,
    }
}

fn normalize_trusted_authorized_parties(
    audience: &str,
    trusted_authorized_parties: Vec<String>,
) -> Vec<String> {
    let mut trusted = trusted_authorized_parties
        .into_iter()
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .collect::<Vec<_>>();
    trusted.push(audience.to_string());
    trusted.sort();
    trusted.dedup();
    trusted
}

fn sha256_hex(input: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input);
    hex::encode(hasher.finalize())
}

fn oidc_device_fingerprint(session_key: &str) -> String {
    sha256_hex(format!("oidc-session:{session_key}").as_bytes())
}

fn form_urlencoded(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(key, value)| {
            format!(
                "{}={}",
                form_percent_encode(key),
                form_percent_encode(value)
            )
        })
        .collect::<Vec<_>>()
        .join("&")
}

fn form_percent_encode(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(byte as char)
            }
            b' ' => encoded.push('+'),
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, routing::get};
    use jsonwebtoken::{EncodingKey, Header, encode, jwk::Jwk};
    use serde_json::json;
    use tokio::net::TcpListener;

    const TEST_ISSUER: &str = "https://ferriskey.test/realms/bibi";
    const TEST_AUDIENCE: &str = "bibi-work";
    const TEST_TRUSTED_AZP: &str = "biwork-desktop";

    const TEST_RSA_KEY_A_PEM: &str = r#"-----BEGIN PRIVATE KEY-----
MIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQDJETqse41HRBsc
7cfcq3ak4oZWFCoZlcic525A3FfO4qW9BMtRO/iXiyCCHn8JhiL9y8j5JdVP2Q9Z
IpfElcFd3/guS9w+5RqQGgCR+H56IVUyHZWtTJbKPcwWXQdNUX0rBFcsBzCRESJL
eelOEdHIjG7LRkx5l/FUvlqsyHDVJEQsHwegZ8b8C0fz0EgT2MMEdn10t6Ur1rXz
jMB/wvCg8vG8lvciXmedyo9xJ8oMOh0wUEgxziVDMMovmC+aJctcHUAYubwoGN8T
yzcvnGqL7JSh36Pwy28iPzXZ2RLhAyJFU39vLaHdljwthUaupldlNyCfa6Ofy4qN
ctlUPlN1AgMBAAECggEAdESTQjQ70O8QIp1ZSkCYXeZjuhj081CK7jhhp/4ChK7J
GlFQZMwiBze7d6K84TwAtfQGZhQ7km25E1kOm+3hIDCoKdVSKch/oL54f/BK6sKl
qlIzQEAenho4DuKCm3I4yAw9gEc0DV70DuMTR0LEpYyXcNJY3KNBOTjN5EYQAR9s
2MeurpgK2MdJlIuZaIbzSGd+diiz2E6vkmcufJLtmYUT/k/ddWvEtz+1DnO6bRHh
xuuDMeJA/lGB/EYloSLtdyCF6sII6C6slJJtgfb0bPy7l8VtL5iDyz46IKyzdyzW
tKAn394dm7MYR1RlUBEfqFUyNK7C+pVMVoTwCC2V4QKBgQD64syfiQ2oeUlLYDm4
CcKSP3RnES02bcTyEDFSuGyyS1jldI4A8GXHJ/lG5EYgiYa1RUivge4lJrlNfjyf
dV230xgKms7+JiXqag1FI+3mqjAgg4mYiNjaao8N8O3/PD59wMPeWYImsWXNyeHS
55rUKiHERtCcvdzKl4u35ZtTqQKBgQDNKnX2bVqOJ4WSqCgHRhOm386ugPHfy+8j
m6cicmUR46ND6ggBB03bCnEG9OtGisxTo/TuYVRu3WP4KjoJs2LD5fwdwJqpgtHl
yVsk45Y1Hfo+7M6lAuR8rzCi6kHHNb0HyBmZjysHWZsn79ZM+sQnLpgaYgQGRbKV
DZWlbw7g7QKBgQCl1u+98UGXAP1jFutwbPsx40IVszP4y5ypCe0gqgon3UiY/G+1
zTLp79GGe/SjI2VpQ7AlW7TI2A0bXXvDSDi3/5Dfya9ULnFXv9yfvH1QwWToySpW
Kvd1gYSoiX84/WCtjZOr0e0HmLIb0vw0hqZA4szJSqoxQgvF22EfIWaIaQKBgQCf
34+OmMYw8fEvSCPxDxVvOwW2i7pvV14hFEDYIeZKW2W1HWBhVMzBfFB5SE8yaCQy
pRfOzj9aKOCm2FjjiErVNpkQoi6jGtLvScnhZAt/lr2TXTrl8OwVkPrIaN0bG/AS
aUYxmBPCpXu3UjhfQiWqFq/mFyzlqlgvuCc9g95HPQKBgAscKP8mLxdKwOgX8yFW
GcZ0izY/30012ajdHY+/QK5lsMoxTnn0skdS+spLxaS5ZEO4qvPVb8RAoCkWMMal
2pOhmquJQVDPDLuZHdrIiKiDM20dy9sMfHygWcZjQ4WSxf/J7T9canLZIXFhHAZT
3wc9h4G8BBCtWN2TN/LsGZdB
-----END PRIVATE KEY-----"#;

    const TEST_RSA_KEY_B_PEM: &str = r#"-----BEGIN RSA PRIVATE KEY-----
MIIEogIBAAKCAQEAnzyis1ZjfNB0bBgKFMSvvkTtwlvBsaJq7S5wA+kzeVOVpVWw
kWdVha4s38XM/pa/yr47av7+z3VTmvDRyAHcaT92whREFpLv9cj5lTeJSibyr/Mr
m/YtjCZVWgaOYIhwrXwKLqPr/11inWsAkfIytvHWTxZYEcXLgAXFuUuaS3uF9gEi
NQwzGTU1v0FqkqTBr4B8nW3HCN47XUu0t8Y0e+lf4s4OxQawWD79J9/5d3Ry0vbV
3Am1FtGJiJvOwRsIfVChDpYStTcHTCMqtvWbV6L11BWkpzGXSW4Hv43qa+GSYOD2
QU68Mb59oSk2OB+BtOLpJofmbGEGgvmwyCI9MwIDAQABAoIBACiARq2wkltjtcjs
kFvZ7w1JAORHbEufEO1Eu27zOIlqbgyAcAl7q+/1bip4Z/x1IVES84/yTaM8p0go
amMhvgry/mS8vNi1BN2SAZEnb/7xSxbflb70bX9RHLJqKnp5GZe2jexw+wyXlwaM
+bclUCrh9e1ltH7IvUrRrQnFJfh+is1fRon9Co9Li0GwoN0x0byrrngU8Ak3Y6D9
D8GjQA4Elm94ST3izJv8iCOLSDBmzsPsXfcCUZfmTfZ5DbUDMbMxRnSo3nQeoKGC
0Lj9FkWcfmLcpGlSXTO+Ww1L7EGq+PT3NtRae1FZPwjddQ1/4V905kyQFLamAA5Y
lSpE2wkCgYEAy1OPLQcZt4NQnQzPz2SBJqQN2P5u3vXl+zNVKP8w4eBv0vWuJJF+
hkGNnSxXQrTkvDOIUddSKOzHHgSg4nY6K02ecyT0PPm/UZvtRpWrnBjcEVtHEJNp
bU9pLD5iZ0J9sbzPU/LxPmuAP2Bs8JmTn6aFRspFrP7W0s1Nmk2jsm0CgYEAyH0X
+jpoqxj4efZfkUrg5GbSEhf+dZglf0tTOA5bVg8IYwtmNk/pniLG/zI7c+GlTc9B
BwfMr59EzBq/eFMI7+LgXaVUsM/sS4Ry+yeK6SJx/otIMWtDfqxsLD8CPMCRvecC
2Pip4uSgrl0MOebl9XKp57GoaUWRWRHqwV4Y6h8CgYAZhI4mh4qZtnhKjY4TKDjx
QYufXSdLAi9v3FxmvchDwOgn4L+PRVdMwDNms2bsL0m5uPn104EzM6w1vzz1zwKz
5pTpPI0OjgWN13Tq8+PKvm/4Ga2MjgOgPWQkslulO/oMcXbPwWC3hcRdr9tcQtn9
Imf9n2spL/6EDFId+Hp/7QKBgAqlWdiXsWckdE1Fn91/NGHsc8syKvjjk1onDcw0
NvVi5vcba9oGdElJX3e9mxqUKMrw7msJJv1MX8LWyMQC5L6YNYHDfbPF1q5L4i8j
8mRex97UVokJQRRA452V2vCO6S5ETgpnad36de3MUxHgCOX3qL382Qx9/THVmbma
3YfRAoGAUxL/Eu5yvMK8SAt/dJK6FedngcM3JEFNplmtLYVLWhkIlNRGDwkg3I5K
y18Ae9n7dHVueyslrb6weq7dTkYDi3iOYRW8HRkIQh06wEdbxt0shTzAJvvCQfrB
jg/3747WSsf/zBTcHihTRBdAv6OmdhV4/dD5YBfLAkLrd+mX7iE=
-----END RSA PRIVATE KEY-----"#;

    const RSA_KEY_A: TestRsaSigningKey = TestRsaSigningKey {
        kid: "rsa-a",
        private_key_pem: TEST_RSA_KEY_A_PEM,
    };
    const RSA_KEY_B: TestRsaSigningKey = TestRsaSigningKey {
        kid: "rsa-b",
        private_key_pem: TEST_RSA_KEY_B_PEM,
    };

    #[derive(Clone, Copy)]
    struct TestRsaSigningKey {
        kid: &'static str,
        private_key_pem: &'static str,
    }

    impl TestRsaSigningKey {
        fn with_kid(self, kid: &'static str) -> Self {
            Self {
                kid,
                private_key_pem: self.private_key_pem,
            }
        }

        fn encoding_key(&self) -> EncodingKey {
            EncodingKey::from_rsa_pem(self.private_key_pem.as_bytes())
                .expect("test RSA private key should parse")
        }

        fn jwk(&self) -> Jwk {
            let encoding_key = self.encoding_key();
            let mut jwk = Jwk::from_encoding_key(&encoding_key, Algorithm::RS256)
                .expect("test RSA private key should export JWK");
            jwk.common.key_id = Some(self.kid.to_string());
            jwk
        }
    }

    #[derive(Clone)]
    struct MockOidcState {
        issuer: String,
        jwks_uri: String,
        jwks: Arc<RwLock<JwkSet>>,
    }

    struct MockOidcServer {
        discovery_url: String,
        jwks: Arc<RwLock<JwkSet>>,
    }

    impl MockOidcServer {
        async fn set_jwks(&self, keys: &[TestRsaSigningKey]) {
            *self.jwks.write().await = jwk_set(keys);
        }
    }

    async fn mock_discovery(State(state): State<MockOidcState>) -> Json<Value> {
        Json(json!({
            "issuer": state.issuer,
            "jwks_uri": state.jwks_uri,
            "authorization_endpoint": format!("{}/authorize", TEST_ISSUER),
            "token_endpoint": format!("{}/token", TEST_ISSUER),
        }))
    }

    async fn mock_jwks(State(state): State<MockOidcState>) -> Json<JwkSet> {
        Json(state.jwks.read().await.clone())
    }

    async fn start_oidc_mock(issuer: &str, keys: &[TestRsaSigningKey]) -> MockOidcServer {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock OIDC listener");
        let base_url = format!(
            "http://{}",
            listener.local_addr().expect("mock listener addr")
        );
        let jwks_uri = format!("{base_url}/jwks");
        let discovery_url = format!("{base_url}/.well-known/openid-configuration");
        let jwks = Arc::new(RwLock::new(jwk_set(keys)));
        let state = MockOidcState {
            issuer: issuer.to_string(),
            jwks_uri,
            jwks: jwks.clone(),
        };
        let app = Router::new()
            .route("/.well-known/openid-configuration", get(mock_discovery))
            .route("/jwks", get(mock_jwks))
            .with_state(state);

        tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("mock OIDC server should run");
        });

        MockOidcServer {
            discovery_url,
            jwks,
        }
    }

    fn jwk_set(keys: &[TestRsaSigningKey]) -> JwkSet {
        JwkSet {
            keys: keys.iter().map(TestRsaSigningKey::jwk).collect(),
        }
    }

    fn test_verifier(discovery_url: &str, issuer: &str) -> FerrisKeyOidcVerifier {
        FerrisKeyOidcVerifier::new(FerrisKeySettings {
            issuer: issuer.to_string(),
            audience: TEST_AUDIENCE.to_string(),
            trusted_authorized_parties: vec![TEST_TRUSTED_AZP.to_string()],
            discovery_url: discovery_url.to_string(),
            jwks_uri: None,
            default_tenant_slug: "default".to_string(),
            timeout_milliseconds: 2_000,
        })
        .expect("test verifier should build")
    }

    fn base_oidc_claims(issuer: &str) -> Value {
        let now = OffsetDateTime::now_utc().unix_timestamp();
        json!({
            "iss": issuer,
            "sub": "ferriskey-user-1",
            "aud": TEST_AUDIENCE,
            "exp": now + 3_600,
            "nbf": now - 60,
            "iat": now,
            "jti": "token-jti-1",
            "sid": "session-1",
            "preferred_username": "aion",
            "email": "aion@example.test",
            "roles": ["admin", " viewer ", ""],
            "realm_access": {
                "roles": ["viewer", "operator"]
            }
        })
    }

    fn signed_oidc_token(key: TestRsaSigningKey, claims: Value) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(key.kid.to_string());
        encode(&header, &claims, &key.encoding_key()).expect("test token should sign")
    }

    async fn rejected_reason(verifier: &FerrisKeyOidcVerifier, token: &str) -> String {
        match verifier.verify_access_token(token).await {
            Ok(_) => panic!("token unexpectedly verified"),
            Err(AppError::Unauthorized(reason)) => reason,
            Err(error) => panic!("unexpected error: {error:?}"),
        }
    }

    #[test]
    fn request_trace_id_prefers_safe_x_trace_id() {
        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static(TRACE_ID_HEADER),
            HeaderValue::from_static("desktop-trace-1"),
        );
        headers.insert(
            HeaderName::from_static("traceparent"),
            HeaderValue::from_static("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-00"),
        );

        assert_eq!(request_trace_id(&headers), "desktop-trace-1");
    }

    #[test]
    fn request_trace_id_falls_back_to_traceparent_trace_id() {
        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("traceparent"),
            HeaderValue::from_static("00-4BF92F3577B34DA6A3CE929D0E0E4736-00f067aa0ba902b7-00"),
        );

        assert_eq!(
            request_trace_id(&headers),
            "4bf92f3577b34da6a3ce929d0e0e4736"
        );
    }

    #[test]
    fn request_trace_id_rejects_unsafe_or_blank_values() {
        assert_eq!(
            normalize_trace_id(" trace:ok/1 "),
            Some("trace:ok/1".to_string())
        );
        assert_eq!(normalize_trace_id(""), None);
        assert_eq!(normalize_trace_id("bad\nheader"), None);
        assert_eq!(
            trace_id_from_traceparent("00-00000000000000000000000000000000-00f067aa0ba902b7-00"),
            None
        );
    }

    #[test]
    fn form_urlencoded_encodes_reserved_values() {
        assert_eq!(
            form_urlencoded(&[("redirect_uri", "http://127.0.0.1:25808/auth/callback")]),
            "redirect_uri=http%3A%2F%2F127.0.0.1%3A25808%2Fauth%2Fcallback"
        );
        assert_eq!(
            form_urlencoded(&[("scope", "openid profile")]),
            "scope=openid+profile"
        );
    }

    #[test]
    fn oidc_device_identity_is_stable_for_one_session() {
        assert_eq!(
            oidc_device_fingerprint("session-1"),
            oidc_device_fingerprint("session-1")
        );
        assert_ne!(
            oidc_device_fingerprint("session-1"),
            oidc_device_fingerprint("session-2")
        );
    }

    #[tokio::test]
    async fn oidc_jwks_verifier_accepts_rs256_token_and_merges_roles() {
        let server = start_oidc_mock(TEST_ISSUER, &[RSA_KEY_A]).await;
        let verifier = test_verifier(&server.discovery_url, TEST_ISSUER);
        let mut claims = base_oidc_claims(TEST_ISSUER);
        claims.as_object_mut().unwrap().remove("aud");
        claims["azp"] = json!(TEST_TRUSTED_AZP);
        let token = signed_oidc_token(RSA_KEY_A, claims);

        let verified = verifier
            .verify_access_token(&token)
            .await
            .expect("trusted FerrisKey token should verify");

        assert_eq!(verified.sub, "ferriskey-user-1");
        assert_eq!(verified.preferred_username.as_deref(), Some("aion"));
        assert_eq!(
            verified.effective_roles(),
            vec![
                "admin".to_string(),
                "operator".to_string(),
                "viewer".to_string()
            ]
        );
    }

    #[tokio::test]
    async fn oidc_jwks_verifier_refreshes_when_kid_or_key_material_rotates() {
        let server = start_oidc_mock(TEST_ISSUER, &[RSA_KEY_A]).await;
        let verifier = test_verifier(&server.discovery_url, TEST_ISSUER);

        let first_token = signed_oidc_token(RSA_KEY_A, base_oidc_claims(TEST_ISSUER));
        verifier
            .verify_access_token(&first_token)
            .await
            .expect("initial token should verify");

        server.set_jwks(&[RSA_KEY_B]).await;
        let second_token = signed_oidc_token(RSA_KEY_B, base_oidc_claims(TEST_ISSUER));
        verifier
            .verify_access_token(&second_token)
            .await
            .expect("new kid should trigger JWKS refresh");

        let rotated_same_kid = RSA_KEY_A.with_kid(RSA_KEY_B.kid);
        server.set_jwks(&[rotated_same_kid]).await;
        let third_token = signed_oidc_token(rotated_same_kid, base_oidc_claims(TEST_ISSUER));
        verifier
            .verify_access_token(&third_token)
            .await
            .expect("signature failure should refresh cached JWKS");
    }

    #[tokio::test]
    async fn oidc_jwks_verifier_rejects_invalid_standard_claims() {
        let server = start_oidc_mock(TEST_ISSUER, &[RSA_KEY_A]).await;
        let verifier = test_verifier(&server.discovery_url, TEST_ISSUER);
        let now = OffsetDateTime::now_utc().unix_timestamp();
        let mut cases = Vec::new();

        let mut wrong_issuer = base_oidc_claims("https://evil.example/realms/bibi");
        wrong_issuer["jti"] = json!("wrong-issuer");
        cases.push(("issuer", wrong_issuer, "invalid FerrisKey access token"));

        let mut expired = base_oidc_claims(TEST_ISSUER);
        expired["exp"] = json!(now - 3_600);
        expired["jti"] = json!("expired");
        cases.push(("expired", expired, "invalid FerrisKey access token"));

        let mut not_before = base_oidc_claims(TEST_ISSUER);
        not_before["nbf"] = json!(now + 3_600);
        not_before["jti"] = json!("nbf");
        cases.push(("nbf", not_before, "invalid FerrisKey access token"));

        let mut wrong_audience = base_oidc_claims(TEST_ISSUER);
        wrong_audience["aud"] = json!("wrong-audience");
        wrong_audience["jti"] = json!("wrong-audience");
        cases.push((
            "audience",
            wrong_audience,
            "FerrisKey token audience mismatch",
        ));

        let mut wrong_azp = base_oidc_claims(TEST_ISSUER);
        wrong_azp.as_object_mut().unwrap().remove("aud");
        wrong_azp["azp"] = json!("untrusted-client");
        wrong_azp["jti"] = json!("wrong-azp");
        cases.push(("azp", wrong_azp, "FerrisKey token audience mismatch"));

        for (name, claims, expected_reason) in cases {
            let token = signed_oidc_token(RSA_KEY_A, claims);
            let reason = rejected_reason(&verifier, &token).await;
            assert!(
                reason.contains(expected_reason),
                "{name} failure reason was {reason:?}"
            );
        }
    }

    #[tokio::test]
    async fn oidc_jwks_verifier_rejects_discovery_issuer_mismatch() {
        let server = start_oidc_mock("https://evil.example/realms/bibi", &[RSA_KEY_A]).await;
        let verifier = test_verifier(&server.discovery_url, TEST_ISSUER);
        let token = signed_oidc_token(RSA_KEY_A, base_oidc_claims(TEST_ISSUER));

        let reason = rejected_reason(&verifier, &token).await;

        assert!(reason.contains("FerrisKey discovery issuer mismatch"));
    }
}
