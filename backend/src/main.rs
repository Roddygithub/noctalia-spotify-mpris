//! Noctalia Spotify Backend - Native Spotify Connect using librespot
//!
//! This backend provides a Spotify Connect device that can be controlled via
//! a Unix socket (JSON lines protocol v1) and serves OAuth on localhost:8000.

mod player;
mod webapi;

use anyhow::{Context, Result};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::UnixListener;
use tokio::signal;
use tracing::{error, info, warn};

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    info!("Starting noctalia-spotify-backend");

    // Get config directory
    let config_dir = dirs::config_dir()
        .context("Could not find config directory")?
        .join("noctalia-spotify-backend");
    std::fs::create_dir_all(&config_dir)?;

    // Initialize database
    let db_path = config_dir.join("cache.db");
    let db_url = format!("sqlite://{}?mode=rwc", db_path.display());
    let pool = sqlx::SqlitePool::connect(&db_url).await?;
    sqlx::migrate!("./migrations").run(&pool).await?;

    // Create web API client
    let webapi = Arc::new(webapi::WebApiClient::new(pool.clone()).await?);

    // Start OAuth server on localhost:8000
    let oauth_router = webapi::oauth_router(webapi.clone());
    let oauth_addr: SocketAddr = "127.0.0.1:8000".parse()?;
    tokio::spawn(async move {
        info!("OAuth server listening on {}", oauth_addr);
        if let Err(e) = axum::serve(
            tokio::net::TcpListener::bind(oauth_addr).await.unwrap(),
            oauth_router,
        )
        .await
        {
            error!("OAuth server error: {}", e);
        }
    });

    // Create player manager
    let (player_tx, player_rx) = async_channel::bounded(32);
    let player_manager = player::PlayerManager::new(webapi.clone(), player_rx).await?;

    // Spawn player manager
    tokio::spawn(async move {
        if let Err(e) = player_manager.run().await {
            error!("Player manager error: {}", e);
        }
    });

    // Unix socket for plugin communication
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR")
        .unwrap_or_else(|_| format!("/run/user/{}", std::process::id()));
    let socket_path = PathBuf::from(runtime_dir)
        .join("noctalia-spotify")
        .join("backend.sock");

    // Remove old socket if exists
    if socket_path.exists() {
        std::fs::remove_file(&socket_path)?;
    }
    std::fs::create_dir_all(socket_path.parent().unwrap())?;

    let listener = UnixListener::bind(&socket_path)?;
    info!("Unix socket listening on {}", socket_path.display());

    // Set permissions to 0700
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o700))?;
    }

    // Handle socket connections
    let socket_tx = player_tx.clone();
    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let tx = socket_tx.clone();
                    tokio::spawn(async move {
                        if let Err(e) = handle_connection(stream, tx).await {
                            warn!("Connection error: {}", e);
                        }
                    });
                }
                Err(e) => {
                    error!("Socket accept error: {}", e);
                }
            }
        }
    });

    // Wait for shutdown signal
    signal::ctrl_c().await?;
    info!("Shutdown signal received");

    // Clean up socket
    let _ = std::fs::remove_file(&socket_path);

    Ok(())
}

async fn handle_connection(
    stream: tokio::net::UnixStream,
    tx: async_channel::Sender<player::PlayerCommand>,
) -> Result<()> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let (rd, mut wr) = stream.into_split();
    let mut reader = BufReader::new(rd);
    let mut line = String::new();

    while reader.read_line(&mut line).await? > 0 {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            line.clear();
            continue;
        }

        // Parse JSON request
        let request: serde_json::Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(e) => {
                let resp = serde_json::json!({
                    "v": 1,
                    "id": 0,
                    "ok": false,
                    "error": { "code": "parse_error", "message": e.to_string() }
                });
                wr.write_all(format!("{}\n", resp).as_bytes()).await?;
                line.clear();
                continue;
            }
        };

        let id = request.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
        let command = request
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let response = match command {
            "get_state" => {
                let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
                if tx
                    .send(player::PlayerCommand::GetState(resp_tx))
                    .await
                    .is_ok()
                {
                    match resp_rx.await {
                        Ok(state) => {
                            serde_json::json!({ "v": 1, "id": id, "ok": true, "data": state })
                        }
                        Err(_) => {
                            serde_json::json!({ "v": 1, "id": id, "ok": false, "error": { "code": "internal_error", "message": "State channel closed" } })
                        }
                    }
                } else {
                    serde_json::json!({ "v": 1, "id": id, "ok": false, "error": { "code": "internal_error", "message": "Command channel closed" } })
                }
            }
            "play_pause" => {
                let _ = tx.send(player::PlayerCommand::PlayPause).await;
                serde_json::json!({ "v": 1, "id": id, "ok": true })
            }
            "next" => {
                let _ = tx.send(player::PlayerCommand::Next).await;
                serde_json::json!({ "v": 1, "id": id, "ok": true })
            }
            "previous" => {
                let _ = tx.send(player::PlayerCommand::Previous).await;
                serde_json::json!({ "v": 1, "id": id, "ok": true })
            }
            "seek" => {
                let position = request
                    .get("args")
                    .and_then(|a| a.get("position"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let _ = tx.send(player::PlayerCommand::Seek(position)).await;
                serde_json::json!({ "v": 1, "id": id, "ok": true })
            }
            "set_volume" => {
                let volume = request
                    .get("args")
                    .and_then(|a| a.get("volume"))
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.5);
                let _ = tx.send(player::PlayerCommand::SetVolume(volume)).await;
                serde_json::json!({ "v": 1, "id": id, "ok": true })
            }
            "shuffle" => {
                let enabled = request
                    .get("args")
                    .and_then(|a| a.get("enabled"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let _ = tx.send(player::PlayerCommand::Shuffle(enabled)).await;
                serde_json::json!({ "v": 1, "id": id, "ok": true })
            }
            "repeat" => {
                let mode = request
                    .get("args")
                    .and_then(|a| a.get("mode"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("None");
                let _ = tx
                    .send(player::PlayerCommand::Repeat(mode.to_string()))
                    .await;
                serde_json::json!({ "v": 1, "id": id, "ok": true })
            }
            "authenticate" => {
                let auth_url = webapi::WebApiClient::auth_url();
                let _ = tx
                    .send(player::PlayerCommand::Authenticate(auth_url.clone()))
                    .await;
                serde_json::json!({ "v": 1, "id": id, "ok": true, "data": { "auth_url": auth_url } })
            }
            "ping" => {
                serde_json::json!({ "v": 1, "id": id, "ok": true })
            }
            _ => {
                serde_json::json!({ "v": 1, "id": id, "ok": false, "error": { "code": "unknown_command", "message": format!("Unknown command: {}", command) } })
            }
        };

        wr.write_all(format!("{}\n", response).as_bytes()).await?;
        line.clear();
    }

    Ok(())
}
