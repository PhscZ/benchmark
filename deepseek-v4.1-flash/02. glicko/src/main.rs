//! Glicko-2 kill-rating REST API.

use clap::Parser;
use glicko_api::config::Config;
use glicko_api::store::Mode;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::parse();
    init_tracing();
    if let Err(message) = config.validate() {
        eprintln!("invalid configuration: {message}");
        std::process::exit(2);
    }

    let started = std::time::Instant::now();
    let store = glicko_api::build_store(&config)?;
    tracing::info!(
        events = store.events().len(),
        players = store.state(Mode::Alt).registered,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "rating state ready"
    );

    let state = glicko_api::build_state(store, config.clone());
    let app = glicko_api::openapi::router(state);

    let address = format!("{}:{}", config.host, config.port);
    let listener = tokio::net::TcpListener::bind(&address).await?;
    tracing::info!("listening on http://{address}");
    tracing::info!("swagger ui on http://{address}/swagger-ui");
    tracing::info!("openapi document on http://{address}/api-docs/openapi.json");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    tracing::info!("shutdown complete");
    Ok(())
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("glicko_api=info,warn"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("install Ctrl+C handler");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
    // Give in-flight requests a moment to finish.
    tokio::time::sleep(Duration::from_millis(50)).await;
}
