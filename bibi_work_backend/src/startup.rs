use axum::{
    Router,
    http::{Method, header},
    middleware,
    routing::get,
};
use redis::Client;
use secrecy::ExposeSecret;
use sqlx::{PgPool, Pool, Postgres, postgres::PgPoolOptions};
use tokio::net::TcpListener;
use tower_http::cors::CorsLayer;

use crate::{
    configuration::{DatabaseSettings, RedisSettings, Settings},
    features::agent_platform,
    features::agent_platform::{
        authz::ResourceAuthzService,
        ferriskey_oidc::{
            FerrisKeyOidcVerifier, ferriskey_access_token_middleware, get_oidc_config,
        },
        internal_auth::internal_token_middleware,
        memory_vector::MemoryVectorClient,
        runtime::AgentRuntimeClient,
        rustfs::RustFsClient,
    },
};

pub fn get_redis_client(redis_settings: RedisSettings) -> Client {
    let info = redis_settings.connection_info();
    Client::open(info).expect("Failed to create Redis client")
}

pub fn get_connection_pool(database_settings: DatabaseSettings) -> PgPool {
    PgPoolOptions::new()
        .acquire_timeout(std::time::Duration::from_millis(
            database_settings.timeout_milliseconds,
        ))
        .connect_lazy_with(database_settings.with_db())
}

pub struct Application {
    port: u16,
    server: Server,
}

pub struct Server {
    listener: TcpListener,
    router: Router,
}

#[derive(Clone)]
pub struct AppState {
    pub connect_pool: Pool<Postgres>,
    pub redis_client: Client,
    pub ferriskey_oidc: FerrisKeyOidcVerifier,
    pub authz_service: ResourceAuthzService,
    pub agent_runtime_client: AgentRuntimeClient,
    pub rustfs_client: RustFsClient,
    pub memory_vector_client: MemoryVectorClient,
    pub internal_shared_token: String,
}

impl Application {
    pub async fn build(configuration: Settings) -> Result<Self, anyhow::Error> {
        let audit_hash_chain_settings = configuration.audit_hash_chain.clone();
        let connect_pool = get_connection_pool(configuration.database);
        let redis_client = get_redis_client(configuration.redis);
        let address = format!(
            "{}:{}",
            configuration.application.host, configuration.application.port
        );
        let listener = TcpListener::bind(address).await?;
        let ferriskey_oidc = FerrisKeyOidcVerifier::new(configuration.ferriskey.clone())?;
        let authz_service = ResourceAuthzService::new(connect_pool.clone());
        let agent_runtime_client = AgentRuntimeClient::new(configuration.agent_runtime)?;
        let rustfs_client = RustFsClient::new(configuration.object_store)?;
        let memory_vector_client = MemoryVectorClient::new(configuration.memory_vector)?;
        let internal_shared_token = configuration
            .internal
            .shared_token
            .expose_secret()
            .to_string();
        let share_state = AppState {
            connect_pool,
            redis_client,
            ferriskey_oidc,
            authz_service,
            agent_runtime_client,
            rustfs_client,
            memory_vector_client,
            internal_shared_token,
        };
        agent_platform::event_store::spawn_outbox_publisher(share_state.clone());
        agent_platform::memory_ingestion::spawn_memory_ingestion_worker(share_state.clone());
        agent_platform::audit_sealing::spawn_audit_hash_chain_sealing_worker(
            share_state.clone(),
            audit_hash_chain_settings,
        );

        let port = listener.local_addr()?.port();

        let cors = CorsLayer::new()
            .allow_origin([
                "http://localhost:34825".parse().unwrap(),
                "http://127.0.0.1:34825".parse().unwrap(),
            ])
            .allow_credentials(true)
            .allow_methods([
                Method::GET,
                Method::POST,
                Method::PUT,
                Method::PATCH,
                Method::DELETE,
            ])
            .allow_headers([header::AUTHORIZATION, header::ACCEPT, header::CONTENT_TYPE]);

        let auth_router = Router::new().route("/oidc/config", get(get_oidc_config));

        let protected_router = Router::new().merge(agent_platform::api_router()).layer(
            middleware::from_fn_with_state(share_state.clone(), ferriskey_access_token_middleware),
        );

        let api_router = Router::new()
            .nest("/auth", auth_router)
            .merge(protected_router);

        let router = Router::new()
            .nest("/api/v1", api_router)
            .nest(
                "/internal",
                agent_platform::internal_router().route_layer(middleware::from_fn_with_state(
                    share_state.clone(),
                    internal_token_middleware,
                )),
            )
            .layer(cors)
            .with_state(share_state);
        let server = Server { listener, router };
        Ok(Self { port, server })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub async fn run_until_stopped(self) -> Result<(), std::io::Error> {
        axum::serve(self.server.listener, self.server.router).await
    }
}
