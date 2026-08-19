//! Spotify Web API client with OAuth and caching

use anyhow::{bail, Context, Result};
use chrono::Utc;
use reqwest::{Client as HttpClient, StatusCode};
use serde::Deserialize;
use sqlx::Pool;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use url::Url;

use crate::webapi::cache::Cache;
use crate::webapi::types::{PlayerState, TokenResponse, UserProfile};

const SPOTIFY_AUTH_URL: &str = "https://accounts.spotify.com/authorize";
const SPOTIFY_TOKEN_URL: &str = "https://accounts.spotify.com/api/token";
const SPOTIFY_API_BASE: &str = "https://api.spotify.com/v1";

const CLIENT_ID_ENV: &str = "SPOTIFY_CLIENT_ID";
// Redirect URI must match Spotify Developer Dashboard.
// Spotify no longer allows localhost aliases — use the loopback
// IP literal explicitly.
const REDIRECT_URI: &str = "http://127.0.0.1:8000/callback";
const SCOPES: &str = "user-read-playback-state user-modify-playback-state user-read-currently-playing streaming user-read-email user-read-private user-library-read user-top-read playlist-read-private playlist-read-collaborative";

/// Resolve the Spotify client ID from SPOTIFY_CLIENT_ID env var,
/// then a config file in the backend config dir.
fn load_client_id() -> Option<String> {
    if let Ok(id) = std::env::var(CLIENT_ID_ENV) {
        let id = id.trim().to_string();
        if !id.is_empty() && id != "your-client-id-here" {
            return Some(id);
        }
    }

    // Fall back to config file
    let config_dir = dirs::config_dir()?.join("noctalia-spotify-backend");
    let config_file = config_dir.join("config.toml");
    if let Ok(contents) = std::fs::read_to_string(&config_file) {
        for line in contents.lines() {
            let line = line.trim();
            if let Some((key, value)) = line.split_once('=') {
                if key.trim() == "client_id" {
                    let id = value.trim().trim_matches('"').to_string();
                    if !id.is_empty() && id != "your-client-id-here" {
                        return Some(id);
                    }
                }
            }
        }
    }

    None
}

/// Path to the backend config file (for documentation / setup)
#[allow(dead_code)]
pub fn config_file_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("noctalia-spotify-backend")
        .join("config.toml")
}

/// Generate a random state for OAuth
fn generate_state() -> String {
    use rand::Rng;
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::thread_rng();
    (0..32)
        .map(|_| CHARSET[rng.gen_range(0..CHARSET.len())] as char)
        .collect()
}

/// Generate PKCE code verifier and challenge
fn generate_pkce() -> (String, String) {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    use rand::Rng;
    use sha2::{Digest, Sha256};

    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";
    let mut rng = rand::thread_rng();
    let verifier: String = (0..128)
        .map(|_| CHARSET[rng.gen_range(0..CHARSET.len())] as char)
        .collect();

    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let challenge = URL_SAFE_NO_PAD.encode(hasher.finalize());

    (verifier, challenge)
}

pub struct WebApiClient {
    http: HttpClient,
    cache: Cache,
    #[allow(dead_code)]
    pool: Pool<sqlx::Sqlite>,
    token: Arc<RwLock<Option<TokenResponse>>>,
    pkce_verifier: Arc<RwLock<Option<String>>>,
    oauth_state: Arc<RwLock<Option<String>>>,
    on_token_updated: Arc<tokio::sync::Mutex<Option<Box<dyn FnOnce(String) + Send + Sync>>>>,
}

impl WebApiClient {
    pub async fn new(pool: Pool<sqlx::Sqlite>) -> Result<Self> {
        let cache = Cache::new(pool.clone()).await?;
        Ok(Self {
            http: HttpClient::new(),
            cache,
            pool,
            token: Arc::new(RwLock::new(None)),
            pkce_verifier: Arc::new(RwLock::new(None)),
            oauth_state: Arc::new(RwLock::new(None)),
            on_token_updated: Arc::new(tokio::sync::Mutex::new(None)),
        })
    }

    /// Set a callback to be invoked when a new access token is obtained.
    /// Called after successful OAuth token exchange or refresh.
    pub async fn set_on_token_updated<F>(&self, callback: F)
    where
        F: FnOnce(String) + Send + Sync + 'static,
    {
        *self.on_token_updated.lock().await = Some(Box::new(callback));
    }

    #[allow(dead_code)]
    pub async fn initialize(&self) -> Result<()> {
        // Load token from cache. Load even if expired so the refresh_token is
        // available to get_valid_token() when it needs to refresh.
        if let Some(token) = self.cache.get_token().await? {
            *self.token.write().await = Some(token);
        }
        Ok(())
    }

    /// Generate an authorization URL and store the PKCE verifier + state.
    /// Must be called on the same instance that later exchanges the code.
    pub async fn auth_url(&self) -> Result<String> {
        let client_id = load_client_id().context(format!(
            "No Spotify client ID configured. Set {} env var or create a config.toml in {}",
            CLIENT_ID_ENV,
            config_file_path().display()
        ))?;

        let (verifier, challenge) = generate_pkce();
        let state = generate_state();

        // Store verifier and state for the callback
        *self.pkce_verifier.write().await = Some(verifier);
        *self.oauth_state.write().await = Some(state.clone());

        let mut url = Url::parse(SPOTIFY_AUTH_URL).unwrap();
        url.query_pairs_mut()
            .append_pair("client_id", &client_id)
            .append_pair("response_type", "code")
            .append_pair("redirect_uri", REDIRECT_URI)
            .append_pair("scope", SCOPES)
            .append_pair("code_challenge_method", "S256")
            .append_pair("code_challenge", &challenge)
            .append_pair("state", &state)
            .append_pair("show_dialog", "true");

        Ok(url.to_string())
    }

    /// Validate the OAuth state parameter to prevent CSRF
    pub async fn validate_state(&self, state: &str) -> bool {
        let expected = self.oauth_state.read().await.clone().unwrap_or_default();
        !expected.is_empty() && expected == state
    }

    pub async fn exchange_code_for_token(&self, code: &str) -> Result<()> {
        let client_id = load_client_id().context("No Spotify client ID configured")?;
        let verifier = self
            .pkce_verifier
            .read()
            .await
            .clone()
            .context("No PKCE verifier. Call auth_url() before exchanging the code.")?;

        let params = [
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", REDIRECT_URI),
            ("client_id", &client_id),
            ("code_verifier", &verifier),
        ];

        let resp = self
            .http
            .post(SPOTIFY_TOKEN_URL)
            .form(&params)
            .send()
            .await?;

        if !resp.status().is_success() {
            let err: serde_json::Value = resp.json().await?;
            bail!("Token exchange failed: {}", err);
        }

        let mut token: TokenResponse = resp.json().await?;
        token.obtained_at = Some(Utc::now());

        self.cache.set_token(&token).await?;
        let access_token = token.access_token.clone();
        *self.token.write().await = Some(token);

        // Clear one-time PKCE verifier and state
        *self.pkce_verifier.write().await = None;
        *self.oauth_state.write().await = None;

        // Invoke token updated callback if set
        if let Some(cb) = self.on_token_updated.lock().await.take() {
            cb(access_token);
        }

        Ok(())
    }

    pub async fn refresh_token(&self) -> Result<()> {
        let client_id = load_client_id().context("No Spotify client ID configured")?;
        let token = self
            .token
            .read()
            .await
            .clone()
            .context("No token to refresh")?;
        let refresh_token = token.refresh_token.context("No refresh token")?;

        let params = [
            ("grant_type", "refresh_token"),
            ("refresh_token", &refresh_token),
            ("client_id", &client_id),
        ];

        let resp = self
            .http
            .post(SPOTIFY_TOKEN_URL)
            .form(&params)
            .send()
            .await?;

        if !resp.status().is_success() {
            let err: serde_json::Value = resp.json().await?;
            anyhow::bail!("Token refresh failed: {}", err);
        }

        let mut new_token: TokenResponse = resp.json().await?;
        new_token.obtained_at = Some(Utc::now());
        // Preserve refresh token if not returned
        if new_token.refresh_token.is_none() {
            new_token.refresh_token = Some(refresh_token);
        }

        self.cache.set_token(&new_token).await?;
        *self.token.write().await = Some(new_token);

        Ok(())
    }

    async fn get_valid_token(&self) -> Result<String> {
        let token_guard = self.token.write().await;

        if let Some(token) = token_guard.as_ref() {
            if !token.is_expired() {
                return Ok(token.access_token.clone());
            }
        }

        // Try refresh
        drop(token_guard);
        self.refresh_token().await?;

        let token = self
            .token
            .read()
            .await
            .clone()
            .context("Token not available after refresh")?;
        Ok(token.access_token)
    }

    /// Get the current access token for librespot engine (does not refresh).
    pub async fn get_access_token(&self) -> Option<String> {
        let token = self.token.read().await;
        token.as_ref().map(|t| t.access_token.clone())
    }

    async fn request<T: for<'de> Deserialize<'de> + 'static>(
        &self,
        method: reqwest::Method,
        endpoint: &str,
        params: Option<&[(&str, &str)]>,
    ) -> Result<T> {
        let token = self.get_valid_token().await?;
        let url = format!("{}{}", SPOTIFY_API_BASE, endpoint);

        let mut req = self.http.request(method.clone(), &url).bearer_auth(&token);

        if let Some(p) = params {
            req = req.query(p);
        }

        // Spotify rejects bodyless PUT/POST with 411 Length Required.
        // Params go in the query string, so the body is always empty here.
        // reqwest's body("") sends chunked encoding; the API needs a real
        // Content-Length: 0 header.
        if matches!(method, reqwest::Method::PUT | reqwest::Method::POST) {
            req = req.header("Content-Length", "0");
        }

        let resp = req.send().await?;

        if resp.status() == StatusCode::UNAUTHORIZED {
            // Token might have expired, try refresh once
            self.refresh_token().await?;
            let new_token = self.get_valid_token().await?;

            let mut req = self.http.request(method.clone(), &url).bearer_auth(&new_token);
            if let Some(p) = params {
                req = req.query(p);
            }
            if matches!(method, reqwest::Method::PUT | reqwest::Method::POST) {
                req = req.header("Content-Length", "0");
            }
            let resp = req.send().await?;
            return Ok(resp.json().await?);
        }

        let status = resp.status();
        if !status.is_success() {
            let err_text = resp.text().await?;
            anyhow::bail!("API error {}: {}", status, err_text);
        }

        // Control commands (play/pause/next/...) expect () and Spotify returns a
        // non-JSON body on success, so skip parsing entirely for the unit type.
        if std::any::TypeId::of::<T>() == std::any::TypeId::of::<()>() {
            return Ok(serde_json::from_str("null")?);
        }

        // Some endpoints return an empty body. serde_json can't parse "" into T,
        // so return the default value in that case.
        let text = resp.text().await?;
        if text.trim().is_empty() {
            return Ok(serde_json::from_str("null")?);
        }
        Ok(serde_json::from_str(&text)?)
    }

    pub async fn get_playback_state(&self) -> Result<Option<PlayerState>> {
        #[derive(Deserialize)]
        struct PlaybackResponse {
            device: Option<serde_json::Value>,
            repeat_state: String,
            shuffle_state: bool,
            #[allow(dead_code)]
            context: Option<serde_json::Value>,
            #[allow(dead_code)]
            timestamp: i64,
            progress_ms: Option<i64>,
            is_playing: bool,
            item: Option<serde_json::Value>,
        }

        let resp: Option<PlaybackResponse> = self
            .request(reqwest::Method::GET, "/me/player", None)
            .await?;

        Ok(resp.map(|r| {
            let item = r.item;
            PlayerState {
                status: if r.is_playing { "Playing" } else { "Paused" }.to_string(),
                title: item
                    .as_ref()
                    .and_then(|i| i.get("name"))
                    .and_then(|n| n.as_str())
                    .unwrap_or("")
                    .to_string(),
                artist: item
                    .as_ref()
                    .and_then(|i| i.get("artists"))
                    .and_then(|a| a.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|a| a.get("name"))
                    .and_then(|n| n.as_str())
                    .unwrap_or("")
                    .to_string(),
                album: item
                    .as_ref()
                    .and_then(|i| i.get("album"))
                    .and_then(|a| a.get("name"))
                    .and_then(|n| n.as_str())
                    .unwrap_or("")
                    .to_string(),
                art_url: item
                    .as_ref()
                    .and_then(|i| i.get("album"))
                    .and_then(|a| a.get("images"))
                    .and_then(|img| img.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|i| i.get("url"))
                    .and_then(|u| u.as_str())
                    .unwrap_or("")
                    .to_string(),
                track_id: item
                    .as_ref()
                    .and_then(|i| i.get("id"))
                    .and_then(|id| id.as_str())
                    .unwrap_or("")
                    .to_string(),
                position: (r.progress_ms.unwrap_or(0) as u64) * 1000,
                duration: item
                    .as_ref()
                    .and_then(|i| i.get("duration_ms"))
                    .and_then(|d| d.as_i64())
                    .unwrap_or(0) as u64
                    * 1000,
                volume: r
                    .device
                    .as_ref()
                    .and_then(|d| d.get("volume_percent"))
                    .and_then(|v| v.as_i64())
                    .unwrap_or(50) as f64
                    / 100.0,
                shuffle: r.shuffle_state,
                loop_status: match r.repeat_state.as_str() {
                    "track" => "Track",
                    "context" => "Playlist",
                    _ => "None",
                }
                .to_string(),
            }
        }))
    }

    pub async fn play_pause(&self, play: bool) -> Result<()> {
        let endpoint = if play {
            "/me/player/play"
        } else {
            "/me/player/pause"
        };
        self.request::<()>(reqwest::Method::PUT, endpoint, None)
            .await
    }

    pub async fn next(&self) -> Result<()> {
        self.request::<()>(reqwest::Method::POST, "/me/player/next", None)
            .await
    }

    pub async fn previous(&self) -> Result<()> {
        self.request::<()>(reqwest::Method::POST, "/me/player/previous", None)
            .await
    }

    pub async fn seek(&self, position_ms: u64) -> Result<()> {
        let pos_str = position_ms.to_string();
        let params = [("position_ms", pos_str.as_str())];
        self.request::<()>(reqwest::Method::PUT, "/me/player/seek", Some(&params))
            .await
    }

    pub async fn set_volume(&self, volume: f64) -> Result<()> {
        let vol = (volume * 100.0).round() as u32;
        let vol_str = vol.to_string();
        let params = [("volume_percent", vol_str.as_str())];
        self.request::<()>(reqwest::Method::PUT, "/me/player/volume", Some(&params))
            .await
    }

    pub async fn set_shuffle(&self, shuffle: bool) -> Result<()> {
        let state_str = shuffle.to_string();
        let params = [("state", state_str.as_str())];
        self.request::<()>(reqwest::Method::PUT, "/me/player/shuffle", Some(&params))
            .await
    }

    pub async fn set_repeat(&self, mode: &str) -> Result<()> {
        let state = match mode {
            "Track" => "track",
            "Playlist" => "context",
            _ => "off",
        };
        let params = [("state", state)];
        self.request::<()>(reqwest::Method::PUT, "/me/player/repeat", Some(&params))
            .await
    }

    #[allow(dead_code)]
    pub async fn get_user_profile(&self) -> Result<UserProfile> {
        self.request(reqwest::Method::GET, "/me", None).await
    }
}
