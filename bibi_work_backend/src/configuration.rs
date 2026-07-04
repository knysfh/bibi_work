use redis::IntoConnectionInfo;
use secrecy::{ExposeSecret, SecretBox};
use serde::Deserialize;
use serde_aux::field_attributes::deserialize_number_from_string;
use sqlx::postgres::{PgConnectOptions, PgSslMode};

#[derive(Deserialize)]
pub struct Settings {
    pub application: ApplicationSettings,
    pub database: DatabaseSettings,
    pub redis: RedisSettings,
    pub ferriskey: FerrisKeySettings,
    pub internal: InternalServiceSettings,
    pub agent_runtime: AgentRuntimeSettings,
    pub object_store: ObjectStoreSettings,
    pub memory_vector: MemoryVectorSettings,
    pub audit_hash_chain: AuditHashChainSettings,
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
