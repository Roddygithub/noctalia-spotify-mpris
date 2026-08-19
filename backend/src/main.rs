//! Noctalia Spotify Backend - Native Spotify Connect using librespot
//!
//! This backend provides a Spotify Connect device that can be controlled via
//! a Unix socket (JSON lines protocol v1) and serves OAuth on localhost:8000.

mod player;
mod webapi;
mod librespot_engine;

use anyhow::{Context, Result};
use librespot_engine::{LibrespotEngine, player_event_to_state};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::UnixListener;
use tokio::signal;
use tokio::sync::{mpsc, RwLock};
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

    // Load cached OAuth token so playback works across restarts
    if let Err(e) = webapi.initialize().await {
        error!("Failed to load cached token: {}", e);
    }

    // Shared state for the librespot engine (started after auth)
    let engine_state = Arc::new(RwLock::new(None::<LibrespotEngine>));
    let engine_state_for_oauth = engine_state.clone();
    let _engine_state_for_player = engine_state.clone();
    let cache_dir = config_dir.join("librespot");
    std::fs::create_dir_all(&cache_dir)?;
    let cache_for_engine = Arc::new(librespot::core::cache::Cache::new(
        Some(&cache_dir),
        Some(&cache_dir),
        Some(&cache_dir),
        None,
    )?);

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

    // Set callback to start librespot engine when OAuth token is obtained
    {
        let engine_state = engine_state_for_oauth.clone();
        let cache = cache_for_engine.clone();
        let webapi = webapi.clone();
        webapi.set_on_token_updated(move |access_token| {
            let engine_state = engine_state.clone();
            let cache = cache.clone();
            tokio::spawn(async move {
                info!("Starting librespot engine after OAuth...");
                match LibrespotEngine::start(
                    access_token,
                    "Noctalia".to_string(),
                    cache,
                )
                .await
                {
                    Ok((engine, mut event_rx)) => {
                        *engine_state.write().await = Some(engine);
                        info!("Librespot engine started successfully after OAuth");
                        tokio::spawn(async move {
                            while let Some(event) = event_rx.recv().await {
                                if let Some((status, position_ms, track_id)) = player_event_to_state(&event) {
                                    info!("Librespot event: {} @ {}ms track={}", status, position_ms, track_id);
                                }
                            }
                        });
                    }
                    Err(e) => {
                        warn!("Failed to start librespot engine: {}", e);
                    }
                }
            });
        }).await;
    }

    // Try to start librespot engine if token is already cached
    {
        let webapi = webapi.clone();
        let engine_state = engine_state_for_oauth.clone();
        let cache = cache_for_engine.clone();
        tokio::spawn(async move {
            // Wait a bit for initialize() to load the token
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            if let Some(token) = webapi.get_access_token().await {
                info!("Starting librespot engine with cached token...");
                match LibrespotEngine::start(
                    token,
                    "Noctalia".to_string(),
                    cache,
                )
                .await
                {
                    Ok((engine, mut event_rx)) => {
                        *engine_state.write().await = Some(engine);
                        info!("Librespot engine started successfully");
                        // Forward player events to update state
                        tokio::spawn(async move {
                            while let Some(event) = event_rx.recv().await {
                                if let Some((status, position_ms, track_id)) = player_event_to_state(&event) {
                                    info!("Librespot event: {} @ {}ms track={}", status, position_ms, track_id);
                                    // TODO: merge with WebAPI state
                                }
                            }
                        });
                    }
                    Err(e) => {
                        warn!("Failed to start librespot engine: {}", e);
                    }
                }
            } else {
                info!("No cached token; librespot engine will start after OAuth");
            }
        });
    }

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
    let socket_webapi = webapi.clone();
    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let tx = socket_tx.clone();
                    let webapi = socket_webapi.clone();
                    tokio::spawn(async move {
                        if let Err(e) = handle_connection(stream, tx, webapi).await {
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
    webapi: std::sync::Arc<webapi::WebApiClient>,
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
            "authenticate" => match webapi.auth_url().await {
                Ok(auth_url) => {
                    let _ = tx
                        .send(player::PlayerCommand::Authenticate(auth_url.clone()))
                        .await;
                    serde_json::json!({ "v": 1, "id": id, "ok": true, "data": { "auth_url": auth_url } })
                }
                Err(e) => serde_json::json!({
                    "v": 1,
                    "id": id,
                    "ok": false,
                    "error": { "code": "config_error", "message": e.to_string() }
                }),
            },
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
