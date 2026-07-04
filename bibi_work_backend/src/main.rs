use anyhow::Ok;
use bibi_work_backend::{
    configuration::get_configuration, startup::Application, telemetry::init_subscriber,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _guard = init_subscriber();

    let configuration = get_configuration().expect("Failed to read configuration.");
    let application = Application::build(configuration).await?;

    tracing::info!("Starting application server");
    application.run_until_stopped().await?;
    tracing::info!("Server shutdown completed");

    Ok(())
}
