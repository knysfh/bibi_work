use std::{collections::HashSet, sync::Arc};

use axum::{
    Json,
    extract::{Request, State},
    http::{HeaderMap, header::AUTHORIZATION},
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
    configuration::FerrisKeySettings, features::core::errors::AppError, startup::AppState,
};

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
    pub token_jti: Option<String>,
    pub token_exp: OffsetDateTime,
}

#[derive(Debug, Deserialize)]
struct OidcDiscoveryDocument {
    issuer: String,
    jwks_uri: String,
    authorization_endpoint: Option<String>,
    token_endpoint: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct OidcPublicConfig {
    issuer: String,
    audience: String,
    authorization_endpoint: Option<String>,
    token_endpoint: Option<String>,
    jwks_uri: String,
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
            jwks_uri: discovery.jwks_uri,
        })
    }

    pub async fn authenticate(
        &self,
        pool: &PgPool,
        headers: &HeaderMap,
        token: &str,
    ) -> Result<PlatformRequestContext, AppError> {
        let claims = self.verify_access_token(token).await?;
        self.upsert_projection(pool, headers, token, claims).await
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
        let device_fingerprint = sha256_hex(format!("oidc:{user_agent}").as_bytes());
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
                last_seen_at
            )
            VALUES ($1, $2, $3, $4, 'oidc', 'standard', CURRENT_TIMESTAMP)
            ON CONFLICT (tenant_id, user_id, device_fingerprint)
            DO UPDATE SET
                device_name = EXCLUDED.device_name,
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
                source_ip, user_agent
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, CURRENT_TIMESTAMP, $10, $11)
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
    let token = bearer_token(request.headers())?;
    let context = state
        .ferriskey_oidc
        .authenticate(&state.connect_pool, request.headers(), token)
        .await?;
    request.extensions_mut().insert(context);
    Ok(next.run(request).await)
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
