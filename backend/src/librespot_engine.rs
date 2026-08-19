//! Librespot Connect engine — runs a Spotify Connect device using the OAuth token.

use anyhow::{Context, Result};
use sha1::Digest;
use librespot::core::authentication::Credentials;
use librespot::core::cache::Cache;
use librespot::core::config::{ConnectConfig, SessionConfig};
use librespot::core::session::Session;
use librespot::discovery::Discovery;
use librespot::playback::audio_backend;
use librespot::playback::config::{AudioFormat, Bitrate, NormalisationMethod, NormalisationType, PlayerConfig, VolumeCtrl};
use librespot::playback::mixer::{find as find_mixer, MixerConfig, MixerFn, NoOpVolume};
use librespot::playback::player::{Player, PlayerEvent};
use librespot::connect::spirc::Spirc;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

pub struct LibrespotEngine {
    session: Session,
    _spirc: Spirc,
    _discovery: Option<Discovery>,
    event_tx: mpsc::UnboundedSender<PlayerEvent>,
}

impl LibrespotEngine {
    /// Start a Connect device using the given OAuth access token.
    /// Returns the engine and a receiver for player events.
    pub async fn start(
        access_token: String,
        device_name: String,
        cache: Arc<Cache>,
    ) -> Result<(Self, mpsc::UnboundedReceiver<PlayerEvent>)> {
        // Build credentials from the OAuth access token (AUTHENTICATION_SPOTIFY_TOKEN = 0x3)
        let credentials = Credentials {
            username: String::new(),
            auth_type: librespot::protocol::authentication::AuthenticationType::AUTHENTICATION_SPOTIFY_TOKEN,
            auth_data: access_token.into_bytes(),
        };

        // Session config
        let mut hasher = sha1::Sha1::default();
        Digest::update(&mut hasher, device_name.as_bytes());
        let device_id = hex::encode(Digest::finalize(hasher));
        let session_config = SessionConfig {
            device_id: device_id.clone(),
            ..Default::default()
        };

        // Connect to Spotify AP
        info!("Connecting librespot session...");
        let (session, _reusable_creds) = Session::connect(session_config, credentials, Some((*cache).clone()), true)
            .await
            .context("Failed to connect librespot session")?;
        info!("Librespot session connected as {}", session.username());

        // Audio backend (rodio)
        let backend_fn = audio_backend::find(Some("rodio".to_string())).context("rodio backend not available")?;

        // Mixer (soft volume)
        let mixer_config = MixerConfig::default();
        let mixer_fn: MixerFn = find_mixer(Some("softvol")).context("softvol mixer not available")?;
        let mixer = mixer_fn(mixer_config);
        let soft_volume = mixer.get_soft_volume();

        // Player config
        let player_config = PlayerConfig {
            bitrate: Bitrate::default(),
            gapless: true,
            normalisation: true,
            normalisation_type: NormalisationType::Auto,
            normalisation_method: NormalisationMethod::Dynamic,
            ..Default::default()
        };

        // Player + event channel
        let (player, event_channel) = Player::new(
            player_config,
            session.clone(),
            soft_volume,
            move || {
                (backend_fn)(None, AudioFormat::default())
            },
        );
        info!("Librespot player created");

        // Spirc (Spotify Connect protocol)
        let connect_config = ConnectConfig {
            name: device_name.clone(),
            device_type: librespot::core::config::DeviceType::Speaker,
            initial_volume: Some(65535), // max volume
            has_volume_ctrl: true,
            autoplay: true,
        };

        let (spirc, spirc_task) = Spirc::new(connect_config, session.clone(), player, mixer);
        info!("Librespot Spirc created");

        // Spawn Spirc task
        tokio::spawn(async move {
            let _ = spirc_task.await;
        });

        // Discovery (zeroconf) — make device appear in Spotify apps
        let discovery = Discovery::builder(device_id)
            .name(device_name)
            .device_type(librespot::core::config::DeviceType::Speaker)
            .launch()
            .context("Failed to launch discovery")?;
        info!("Librespot discovery launched");

        // Forward player events
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let event_tx_clone = event_tx.clone();
        tokio::spawn(async move {
            let mut events = event_channel;
            while let Some(event) = events.recv().await {
                if event_tx_clone.send(event).is_err() {
                    break; // receiver dropped
                }
            }
        });

        Ok((
            Self {
                session,
                _spirc: spirc,
                _discovery: Some(discovery),
                event_tx,
            },
            event_rx,
        ))
    }

    /// Get a reference to the Spirc for Connect commands.
    pub fn spirc(&self) -> &Spirc {
        &self._spirc
    }
}

/// Convert PlayerEvent to a simple playback state update.
/// Returns (status, position_ms, track_id_hex) or None if not a state-changing event.
pub fn player_event_to_state(event: &PlayerEvent) -> Option<(&'static str, u32, String)> {
    use librespot::playback::player::PlayerEvent::*;
    match event {
        Playing { track_id, position_ms, .. } => {
            Some(("Playing", *position_ms, track_id.to_base16().unwrap_or_default()))
        }
        Paused { track_id, position_ms, .. } => {
            Some(("Paused", *position_ms, track_id.to_base16().unwrap_or_default()))
        }
        Stopped { track_id, .. } => {
            Some(("Stopped", 0, track_id.to_base16().unwrap_or_default()))
        }
        Loading { track_id, position_ms, .. } => {
            Some(("Loading", *position_ms, track_id.to_base16().unwrap_or_default()))
        }
        _ => None,
    }
}