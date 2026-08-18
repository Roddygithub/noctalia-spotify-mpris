//! Player manager using Spotify Web API for playback control
//!
//! This is a simplified version that doesn't use librespot directly.
//! For full Spotify Connect support, librespot integration would be needed.

use anyhow::{Context, Result};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::interval;
use tracing::{debug, error, info, warn};

use crate::webapi::WebApiClient;
use crate::webapi::types::PlayerState;

#[derive(Debug)]
pub enum PlayerCommand {
    GetState(tokio::sync::oneshot::Sender<Option<PlayerState>>),
    PlayPause,
    Next,
    Previous,
    Seek(u64), // microseconds
    SetVolume(f64),
    Shuffle(bool),
    Repeat(String),
    Authenticate(String),
}

pub struct PlayerManager {
    webapi: Arc<WebApiClient>,
    rx: async_channel::Receiver<PlayerCommand>,
    playback_state: Arc<tokio::sync::RwLock<Option<PlayerState>>>,
}

impl PlayerManager {
    pub async fn new(webapi: Arc<WebApiClient>, rx: async_channel::Receiver<PlayerCommand>) -> Result<Self> {
        let playback_state = Arc::new(tokio::sync::RwLock::new(None));

        Ok(Self {
            webapi,
            rx,
            playback_state,
        })
    }

    pub async fn run(mut self) -> Result<()> {
        // Start playback state polling
        let webapi = self.webapi.clone();
        let playback_state = self.playback_state.clone();
        tokio::spawn(async move {
            let mut interval = interval(Duration::from_secs(2));
            loop {
                interval.tick().await;
                if let Ok(Some(state)) = webapi.get_playback_state().await {
                    *playback_state.write().await = Some(state);
                }
            }
        });

        // Handle commands
        while let Ok(cmd) = self.rx.recv().await {
            match cmd {
                PlayerCommand::GetState(tx) => {
                    let state = self.playback_state.read().await.clone();
                    let _ = tx.send(state);
                }
                PlayerCommand::PlayPause => {
                    let state = self.playback_state.read().await.clone();
                    let play = state.as_ref().map(|s| s.status != "Playing").unwrap_or(true);
                    if let Err(e) = self.webapi.play_pause(play).await {
                        error!("Play/pause failed: {}", e);
                    }
                }
                PlayerCommand::Next => {
                    if let Err(e) = self.webapi.next().await {
                        error!("Next failed: {}", e);
                    }
                }
                PlayerCommand::Previous => {
                    if let Err(e) = self.webapi.previous().await {
                        error!("Previous failed: {}", e);
                    }
                }
                PlayerCommand::Seek(pos) => {
                    let pos_ms = pos / 1000;
                    if let Err(e) = self.webapi.seek(pos_ms).await {
                        error!("Seek failed: {}", e);
                    }
                }
                PlayerCommand::SetVolume(vol) => {
                    if let Err(e) = self.webapi.set_volume(vol).await {
                        error!("Set volume failed: {}", e);
                    }
                }
                PlayerCommand::Shuffle(enabled) => {
                    if let Err(e) = self.webapi.set_shuffle(enabled).await {
                        error!("Set shuffle failed: {}", e);
                    }
                }
                PlayerCommand::Repeat(mode) => {
                    if let Err(e) = self.webapi.set_repeat(&mode).await {
                        error!("Set repeat failed: {}", e);
                    }
                }
                PlayerCommand::Authenticate(_auth_url) => {
                    info!("Authentication requested - handled by OAuth server");
                }
            }
        }

        Ok(())
    }
}