use std::time::Duration;

use readlater_backend::{AppState, app, auth, cli, config::Config, db, images};
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

    // Betreiberbefehl: Passwort neu setzen (siehe cli.rs).
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("reset-credentials") {
        std::process::exit(reset_credentials_command(&config, &args[1..]).await);
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

    let state = match AppState::new(pool, config) {
        Ok(state) => state,
        Err(e) => {
            tracing::error!("Anwendung konnte nicht gestartet werden: {e}");
            std::process::exit(1);
        }
    };

    // Erholte Zähler der Ratenbegrenzung jede Minute entfernen.
    let limits = state.limits.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;
            limits.purge();
        }
    });

    // Abgelaufene Sessions stündlich entfernen.
    let purge_state = state.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(3600));
        loop {
            interval.tick().await;
            auth::purge_expired_sessions(&purge_state).await;
        }
    });

    // Bilder der Artikel im Hintergrund holen und verkleinern (Warteschlange in der Datenbank).
    if state.config.image_store_enabled {
        tokio::spawn(images::run_worker(state.clone()));
    }

    let listener = match TcpListener::bind(&state.config.bind_addr).await {
        Ok(listener) => listener,
        Err(e) => {
            tracing::error!("Konnte {} nicht binden: {e}", state.config.bind_addr);
            std::process::exit(1);
        }
    };
    tracing::info!(
        "Backend lauscht auf {} (Registrierung: {}, Passkeys: {})",
        state.config.bind_addr,
        if state.config.registration_enabled {
            "offen"
        } else {
            "geschlossen"
        },
        if state.passkeys.is_some() {
            "an"
        } else {
            "aus"
        }
    );

    // `ConnectInfo` liefert die Adresse der direkten Verbindung (Rückfall, wenn kein
    // `X-Forwarded-For` vom Proxy kommt).
    if let Err(e) = axum::serve(
        listener,
        app(state).into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
    {
        tracing::error!("Server beendet mit Fehler: {e}");
        std::process::exit(1);
    }
}

/// `reset-credentials <E-Mail>`: fragt zweimal nach dem neuen Passwort (ohne Echo) und setzt es.
/// Gibt den Exit-Code zurück.
async fn reset_credentials_command(config: &Config, args: &[String]) -> i32 {
    let parsed = match cli::parse_reset_args(args) {
        Ok(parsed) => parsed,
        Err(message) => {
            eprintln!("{message}\n\n{}", cli::RESET_USAGE);
            return 2;
        }
    };
    // Eine fehlende Datei nicht stillschweigend neu anlegen: Dann stimmt DATABASE_PATH nicht.
    if !std::path::Path::new(&config.database_path).exists() {
        eprintln!("Die Datenbank {} existiert nicht", config.database_path);
        return 1;
    }
    let password = match read_new_password(parsed.password_stdin) {
        Ok(password) => password,
        Err(message) => {
            eprintln!("{message}");
            return 1;
        }
    };
    let pool = match db::connect(&config.database_path).await {
        Ok(pool) => pool,
        Err(e) => {
            eprintln!("Datenbank konnte nicht geöffnet werden: {e}");
            return 1;
        }
    };
    let options = cli::ResetOptions {
        email: parsed.email.clone(),
        new_password: password,
        revoke_passkeys: parsed.revoke_passkeys,
        revoke_share_tokens: parsed.revoke_share_tokens,
    };
    match cli::reset_credentials(&pool, &options).await {
        Ok(report) => {
            println!(
                "Passwort für {} gesetzt.",
                parsed.email.trim().to_lowercase()
            );
            println!("Beendete Sitzungen: {}", report.sessions_ended);
            if parsed.revoke_passkeys {
                println!("Gelöschte Passkeys: {}", report.passkeys_removed);
            }
            if parsed.revoke_share_tokens {
                println!(
                    "Gelöschte Tokens zum Teilen: {}",
                    report.share_tokens_removed
                );
            }
            0
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

/// Liest das neue Passwort: aus der Standardeingabe (für Skripte) oder mit Rückfrage ohne Echo.
fn read_new_password(from_stdin: bool) -> Result<String, String> {
    if from_stdin {
        let mut line = String::new();
        std::io::stdin()
            .read_line(&mut line)
            .map_err(|e| format!("Eingabe konnte nicht gelesen werden: {e}"))?;
        return Ok(line.trim_end_matches(['\r', '\n']).to_owned());
    }
    let first = rpassword::prompt_password("Neues Passwort: ")
        .map_err(|e| format!("Eingabe konnte nicht gelesen werden: {e}"))?;
    let second = rpassword::prompt_password("Wiederholen: ")
        .map_err(|e| format!("Eingabe konnte nicht gelesen werden: {e}"))?;
    if first != second {
        return Err("Die Passwörter stimmen nicht überein".into());
    }
    Ok(first)
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
