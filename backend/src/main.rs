use std::{sync::Arc, time::Duration};

use readlater_backend::{AppState, app, auth, config::Config, db};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let config = Config::from_env();

    // Für den Docker-Healthcheck (das Image enthält weder curl noch eine Shell).
    if std::env::args().any(|arg| arg == "--healthcheck") {
        std::process::exit(if healthcheck(&config.bind_addr).await {
            0
        } else {
            1
        });
    }

    let pool = match db::connect(&config.database_path).await {
        Ok(pool) => pool,
        Err(e) => {
            tracing::error!(
                "Datenbank {} konnte nicht geöffnet werden: {e}",
                config.database_path
            );
            std::process::exit(1);
        }
    };

    let state = AppState {
        pool,
        config: Arc::new(config),
    };

    // Abgelaufene Sessions stündlich entfernen.
    let purge_state = state.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(3600));
        loop {
            interval.tick().await;
            auth::purge_expired_sessions(&purge_state).await;
        }
    });

    let listener = match TcpListener::bind(&state.config.bind_addr).await {
        Ok(listener) => listener,
        Err(e) => {
            tracing::error!("Konnte {} nicht binden: {e}", state.config.bind_addr);
            std::process::exit(1);
        }
    };
    tracing::info!(
        "Backend lauscht auf {} (Registrierung: {})",
        state.config.bind_addr,
        if state.config.registration_enabled {
            "offen"
        } else {
            "geschlossen"
        }
    );

    if let Err(e) = axum::serve(listener, app(state))
        .with_graceful_shutdown(shutdown_signal())
        .await
    {
        tracing::error!("Server beendet mit Fehler: {e}");
        std::process::exit(1);
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut sig) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            sig.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("Beende kontrolliert …");
}

/// Minimaler HTTP-Check gegen `/api/health` auf dem lokalen Port.
async fn healthcheck(bind_addr: &str) -> bool {
    let port = bind_addr.rsplit(':').next().unwrap_or("3000");
    let check = async {
        let mut stream = TcpStream::connect(format!("127.0.0.1:{port}")).await.ok()?;
        stream
            .write_all(b"GET /api/health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .ok()?;
        let mut response = String::new();
        stream.read_to_string(&mut response).await.ok()?;
        Some(response.starts_with("HTTP/1.1 200"))
    };
    matches!(
        tokio::time::timeout(Duration::from_secs(3), check).await,
        Ok(Some(true))
    )
}
