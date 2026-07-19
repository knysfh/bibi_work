use redis::IntoConnectionInfo;
use secrecy::{ExposeSecret, SecretBox};
use serde::Deserialize;
use serde_aux::field_attributes::deserialize_number_from_string;
use sqlx::postgres::{PgConnectOptions, PgSslMode};

#[derive(Deserialize)]
pub struct Settings {
    pub application: ApplicationSettings,
    pub telemetry: TelemetrySettings,
    pub database: DatabaseSettings,
    pub redis: RedisSettings,
    pub ferriskey: FerrisKeySettings,
    pub internal: InternalServiceSettings,
    pub agent_runtime: AgentRuntimeSettings,
    pub object_store: ObjectStoreSettings,
    pub memory_vector: MemoryVectorSettings,
    pub audit_hash_chain: AuditHashChainSettings,
    pub audit_archive: AuditArchiveSettings,
    pub audit_partition: AuditPartitionSettings,
    pub secret_resolver: SecretResolverSettings,
    pub credential_rotation: CredentialRotationSettings,
    pub mcp_health: McpHealthSettings,
}

#[derive(Deserialize, Clone)]
pub struct TelemetrySettings {
    pub otlp_enabled: bool,
    pub otlp_endpoint: Option<String>,
    pub service_name: String,
    pub trace_sample_ratio: f64,
    pub timeout_milliseconds: u64,
}

impl TelemetrySettings {
    pub fn validate(&self) -> Result<(), anyhow::Error> {
        anyhow::ensure!(
            (0.0..=1.0).contains(&self.trace_sample_ratio),
            "telemetry.trace_sample_ratio must be between 0 and 1"
        );
        anyhow::ensure!(
            !self.service_name.trim().is_empty(),
            "telemetry.service_name must not be empty"
        );
        anyhow::ensure!(
            self.timeout_milliseconds > 0,
            "telemetry.timeout_milliseconds must be greater than zero"
        );
        if self.otlp_enabled {
            anyhow::ensure!(
                self.otlp_endpoint
                    .as_deref()
                    .is_some_and(|endpoint| !endpoint.trim().is_empty()),
                "telemetry.otlp_endpoint is required when OTLP is enabled"
            );
        }
        Ok(())
    }

    pub fn timeout(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.timeout_milliseconds)
    }
}

#[derive(Deserialize)]
pub struct ApplicationSettings {
    #[serde(deserialize_with = "deserialize_number_from_string")]
    pub port: u16,
    pub host: String,
}

#[derive(Deserialize)]
pub struct DatabaseSettings {
    pub username: String,
    pub password: SecretBox<str>,
    #[serde(deserialize_with = "deserialize_number_from_string")]
    pub port: u16,
    pub host: String,
    pub database_name: String,
    pub require_ssl: bool,
    pub timeout_milliseconds: u64,
}

impl DatabaseSettings {
    pub fn without_db(&self) -> PgConnectOptions {
        let ssl_mode = if self.require_ssl {
            PgSslMode::Require
        } else {
            PgSslMode::Prefer
        };

        PgConnectOptions::new()
            .host(&self.host)
            .port(self.port)
            .username(&self.username)
            .password(self.password.expose_secret())
            .ssl_mode(ssl_mode)
    }

    pub fn with_db(&self) -> PgConnectOptions {
        self.without_db().database(&self.database_name)
    }
}

#[derive(Deserialize, Clone)]
pub struct RedisSettings {
    pub url: SecretBox<str>,
}

impl RedisSettings {
    pub fn connection_info(&self) -> redis::ConnectionInfo {
        self.url.expose_secret().into_connection_info().unwrap()
    }
}

#[derive(Deserialize, Clone)]
pub struct FerrisKeySettings {
    pub issuer: String,
    pub audience: String,
    #[serde(default)]
    pub trusted_authorized_parties: Vec<String>,
    pub discovery_url: String,
    pub jwks_uri: Option<String>,
    pub default_tenant_slug: String,
    pub timeout_milliseconds: u64,
}

#[derive(Deserialize, Clone)]
pub struct InternalServiceSettings {
    pub shared_token: SecretBox<str>,
}

#[derive(Deserialize, Clone)]
pub struct AgentRuntimeSettings {
    pub base_url: Option<String>,
    pub shared_token: SecretBox<str>,
    pub timeout_milliseconds: u64,
}

#[derive(Deserialize, Clone)]
pub struct McpHealthSettings {
    pub worker_enabled: bool,
    pub interval_seconds: u64,
    pub batch_size: i64,
}

impl McpHealthSettings {
    pub fn interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.interval_seconds.clamp(10, 86_400))
    }

    pub fn batch_size(&self) -> i64 {
        self.batch_size.clamp(1, 500)
    }
}

#[derive(Deserialize, Clone)]
pub struct ObjectStoreSettings {
    pub enabled: bool,
    pub endpoint: String,
    pub access_key: SecretBox<str>,
    pub secret_key: SecretBox<str>,
    pub region: String,
    pub files_bucket: String,
    pub audit_bucket: String,
    pub timeout_milliseconds: u64,
}

#[derive(Deserialize, Clone)]
pub struct MemoryVectorSettings {
    pub enabled: bool,
    pub embedding_endpoint: Option<String>,
    pub qdrant_rest_url: Option<String>,
    pub qdrant_collection: String,
    pub timeout_milliseconds: u64,
    pub max_context_chars: usize,
    pub worker_interval_milliseconds: u64,
    pub worker_batch_size: i64,
    pub worker_max_attempts: i32,
}

#[derive(Deserialize, Clone)]
pub struct AuditHashChainSettings {
    pub auto_seal_enabled: bool,
    pub worker_interval_milliseconds: u64,
    pub worker_tenant_batch_size: i64,
    pub segment_max_rows: i64,
}

impl AuditHashChainSettings {
    pub fn worker_interval_milliseconds(&self) -> u64 {
        self.worker_interval_milliseconds.max(1_000)
    }

    pub fn worker_tenant_batch_size(&self) -> i64 {
        self.worker_tenant_batch_size.clamp(1, 1_000)
    }

    pub fn segment_max_rows(&self) -> i64 {
        self.segment_max_rows.clamp(1, 10_000)
    }
}

#[derive(Deserialize, Clone)]
pub struct AuditArchiveSettings {
    pub enabled: bool,
    pub worker_interval_milliseconds: u64,
    pub segment_batch_size: i64,
    pub minimum_age_days: i64,
    pub retention_days: i64,
    pub max_attempts: i32,
}

impl AuditArchiveSettings {
    pub fn worker_interval_milliseconds(&self) -> u64 {
        self.worker_interval_milliseconds.max(1_000)
    }

    pub fn segment_batch_size(&self) -> i64 {
        self.segment_batch_size.clamp(1, 1_000)
    }

    pub fn minimum_age_days(&self) -> i64 {
        self.minimum_age_days.clamp(0, 36_500)
    }

    pub fn retention_days(&self) -> i64 {
        self.retention_days.clamp(1, 36_500)
    }

    pub fn max_attempts(&self) -> i32 {
        self.max_attempts.clamp(1, 100)
    }
}

#[derive(Deserialize, Clone)]
pub struct AuditPartitionSettings {
    pub maintenance_enabled: bool,
    pub worker_interval_milliseconds: u64,
    pub months_ahead: i32,
    pub cleanup_enabled: bool,
}

impl AuditPartitionSettings {
    pub fn worker_interval_milliseconds(&self) -> u64 {
        self.worker_interval_milliseconds.max(60_000)
    }

    pub fn months_ahead(&self) -> i32 {
        self.months_ahead.clamp(1, 24)
    }
}

#[derive(Deserialize, Clone)]
pub struct SecretResolverSettings {
    pub timeout_milliseconds: u64,
    pub vault_enabled: bool,
    pub vault_base_url: Option<String>,
    pub vault_token_ref: Option<String>,
    pub vault_namespace: Option<String>,
    pub kms_enabled: bool,
    pub kms_base_url: Option<String>,
    pub kms_auth_token_ref: Option<String>,
    pub rotation_gateway_enabled: bool,
    pub rotation_gateway_base_url: Option<String>,
    pub rotation_gateway_auth_token_ref: Option<String>,
}

#[derive(Deserialize, Clone)]
pub struct CredentialRotationSettings {
    pub worker_enabled: bool,
    pub worker_interval_milliseconds: u64,
    pub batch_size: i64,
    pub stale_claim_seconds: i64,
}

impl CredentialRotationSettings {
    pub fn worker_interval_milliseconds(&self) -> u64 {
        self.worker_interval_milliseconds.max(1_000)
    }

    pub fn batch_size(&self) -> i64 {
        self.batch_size.clamp(1, 100)
    }

    pub fn stale_claim_seconds(&self) -> i64 {
        self.stale_claim_seconds.clamp(60, 3_600)
    }
}

impl SecretResolverSettings {
    pub fn timeout_milliseconds(&self) -> u64 {
        self.timeout_milliseconds.clamp(100, 30_000)
    }
}

pub fn get_configuration() -> Result<Settings, config::ConfigError> {
    let base_path = std::env::current_dir().expect("Failed to determine the current directory");
    let configuration_directory = base_path.join("configuration");

    let environment: Environment = std::env::var("APP_ENVIRONMENT")
        .unwrap_or_else(|_| "local".to_owned())
        .try_into()
        .expect("Failed to parse APP_ENVIRONMENT.");
    let environment_filename = format!("{}.yaml", environment.as_str());

    let settings = config::Config::builder()
        .add_source(config::File::from(
            configuration_directory.join("base.yaml"),
        ))
        .add_source(config::File::from(
            configuration_directory.join(environment_filename),
        ))
        .add_source(
            config::Environment::with_prefix("APP")
                .prefix_separator("_")
                .separator("__"),
        )
        .build()?;

    settings.try_deserialize::<Settings>()
}

pub enum Environment {
    Local,
    Production,
}

impl Environment {
    pub fn as_str(&self) -> &'static str {
        match self {
            Environment::Local => "local",
            Environment::Production => "production",
        }
    }
}

impl TryFrom<String> for Environment {
    type Error = String;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        match value.to_lowercase().as_str() {
            "local" => Ok(Environment::Local),
            "production" => Ok(Environment::Production),
            other => Err(format!(
                "{} is not a supported environment. Use either `local` or `production`.",
                other
            )),
        }
    }
}
