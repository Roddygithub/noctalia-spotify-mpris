//! Spotify Web API client with OAuth and caching

use anyhow::{Context, Result};
use axum::extract::State;
use axum::response::IntoResponse;
use chrono::{DateTime, Duration, Utc};
use reqwest::{Client as HttpClient, StatusCode};
use serde::{Deserialize, Serialize};
use sqlx::Pool;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use url::Url;

use crate::webapi::cache::Cache;
use crate::webapi::types::{CachedResponse, PlayerState, TokenResponse, UserProfile};

const SPOTIFY_AUTH_URL: &str = "https://accounts.spotify.com/authorize";
const SPOTIFY_TOKEN_URL: &str = "https://accounts.spotify.com/api/token";
const SPOTIFY_API_BASE: &str = "https://api.spotify.com/v1";

// Client ID for the Noctalia Spotify app (public, can be rotated)
const CLIENT_ID: &str = "your-client-id-here";
// Redirect URI must match Spotify Developer Dashboard
const REDIRECT_URI: &str = "http://localhost:8000/callback";
const SCOPES: &str = "user-read-playback-state user-modify-playback-state user-read-currently-playing streaming user-read-email user-read-private user-library-read user-top-read playlist-read-private playlist-read-collaborative";

/// Generate a random state for OAuth
fn generate_state() -> String {
    use rand::Rng;
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::thread_rng();
    (0..32).map(|_| CHARSET[rng.gen_range(0..CHARSET.len())] as char).collect()
}

/// Generate PKCE code verifier and challenge
fn generate_pkce() -> (String, String) {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    use rand::Rng;
    use sha2::{Digest, Sha256};

    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";
    let mut rng = rand::thread_rng();
    let verifier: String = (0..128).map(|_| CHARSET[rng.gen_range(0..CHARSET.len())] as char).collect();

    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let challenge = URL_SAFE_NO_PAD.encode(hasher.finalize());

    (verifier, challenge)
}

pub struct WebApiClient {
    http: HttpClient,
    cache: Cache,
    pool: Pool<sqlx::Sqlite>,
    token: Arc<RwLock<Option<TokenResponse>>>,
    pkce_verifier: Arc<RwLock<Option<String>>>,
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
        })
    }

    pub async fn initialize(&self) -> Result<()> {
        // Load token from cache
        if let Some(token) = self.cache.get_token().await? {
            if !token.is_expired() {
                *self.token.write().await = Some(token);
            }
        }
        Ok(())
    }

    pub fn auth_url() -> String {
        let (_, challenge) = generate_pkce();
        let state = generate_state();

        let mut url = Url::parse(SPOTIFY_AUTH_URL).unwrap();
        url.query_pairs_mut()
            .append_pair("client_id", CLIENT_ID)
            .append_pair("response_type", "code")
            .append_pair("redirect_uri", REDIRECT_URI)
            .append_pair("scope", SCOPES)
            .append_pair("code_challenge_method", "S256")
            .append_pair("code_challenge", &challenge)
            .append_pair("state", &state)
            .append_pair("show_dialog", "true");

        url.to_string()
    }

    pub async fn exchange_code_for_token(&self, code: &str) -> Result<()> {
        let verifier = self.pkce_verifier.read().await.clone().context("No PKCE verifier")?;

        let params = [
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", REDIRECT_URI),
            ("client_id", CLIENT_ID),
            ("code_verifier", &verifier),
        ];

        let resp = self.http
            .post(SPOTIFY_TOKEN_URL)
            .form(&params)
            .send()
            .await?;

        if !resp.status().is_success() {
            let err: serde_json::Value = resp.json().await?;
            anyhow::bail!("Token exchange failed: {}", err);
        }

        let mut token: TokenResponse = resp.json().await?;
        token.obtained_at = Some(Utc::now());

        self.cache.set_token(&token).await?;
        *self.token.write().await = Some(token);

        Ok(())
    }

    pub async fn refresh_token(&self) -> Result<()> {
        let token = self.token.read().await.clone().context("No token to refresh")?;
        let refresh_token = token.refresh_token.context("No refresh token")?;

        let params = [
            ("grant_type", "refresh_token"),
            ("refresh_token", &refresh_token),
            ("client_id", CLIENT_ID),
        ];

        let resp = self.http
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
        let mut token_guard = self.token.write().await;

        if let Some(token) = token_guard.as_ref() {
            if !token.is_expired() {
                return Ok(token.access_token.clone());
            }
        }

        // Try refresh
        drop(token_guard);
        self.refresh_token().await?;

        let token = self.token.read().await.clone().context("Token not available after refresh")?;
        Ok(token.access_token)
    }

    async fn request<T: for<'de> Deserialize<'de>>(&self, method: reqwest::Method, endpoint: &str, params: Option<&[(&str, &str)]>) -> Result<T> {
        let token = self.get_valid_token().await?;
        let url = format!("{}{}", SPOTIFY_API_BASE, endpoint);

        let mut req = self.http.request(method.clone(), &url).bearer_auth(&token);

        if let Some(p) = params {
            req = req.query(p);
        }

        let resp = req.send().await?;

        if resp.status() == StatusCode::UNAUTHORIZED {
            // Token might have expired, try refresh once
            self.refresh_token().await?;
            let new_token = self.get_valid_token().await?;

            let mut req = self.http.request(method, &url).bearer_auth(&new_token);
            if let Some(p) = params {
                req = req.query(p);
            }
            let resp = req.send().await?;
            return Ok(resp.json().await?);
        }

        let status = resp.status();
        if !status.is_success() {
            let err_text = resp.text().await?;
            anyhow::bail!("API error {}: {}", status, err_text);
        }

        Ok(resp.json().await?)
    }

    pub async fn get_playback_state(&self) -> Result<Option<PlayerState>> {
        #[derive(Deserialize)]
        struct PlaybackResponse {
            device: Option<serde_json::Value>,
            repeat_state: String,
            shuffle_state: bool,
            context: Option<serde_json::Value>,
            timestamp: i64,
            progress_ms: Option<i64>,
            is_playing: bool,
            item: Option<serde_json::Value>,
        }

        let resp: Option<PlaybackResponse> = self.request(reqwest::Method::GET, "/me/player", None).await?;

        Ok(resp.map(|r| {
            let item = r.item;
            PlayerState {
                status: if r.is_playing { "Playing" } else { "Paused" }.to_string(),
                title: item.as_ref().and_then(|i| i.get("name")).and_then(|n| n.as_str()).unwrap_or("").to_string(),
                artist: item.as_ref().and_then(|i| i.get("artists")).and_then(|a| a.as_array()).and_then(|arr| arr.first()).and_then(|a| a.get("name")).and_then(|n| n.as_str()).unwrap_or("").to_string(),
                album: item.as_ref().and_then(|i| i.get("album")).and_then(|a| a.get("name")).and_then(|n| n.as_str()).unwrap_or("").to_string(),
                art_url: item.as_ref().and_then(|i| i.get("album")).and_then(|a| a.get("images")).and_then(|img| img.as_array()).and_then(|arr| arr.first()).and_then(|i| i.get("url")).and_then(|u| u.as_str()).unwrap_or("").to_string(),
                track_id: item.as_ref().and_then(|i| i.get("id")).and_then(|id| id.as_str()).unwrap_or("").to_string(),
                position: (r.progress_ms.unwrap_or(0) as u64) * 1000,
                duration: item.as_ref().and_then(|i| i.get("duration_ms")).and_then(|d| d.as_i64()).unwrap_or(0) as u64 * 1000,
                volume: r.device.as_ref().and_then(|d| d.get("volume_percent")).and_then(|v| v.as_i64()).unwrap_or(50) as f64 / 100.0,
                shuffle: r.shuffle_state,
                loop_status: match r.repeat_state.as_str() {
                    "track" => "Track",
                    "context" => "Playlist",
                    _ => "None",
                }.to_string(),
            }
        }))
    }

    pub async fn play_pause(&self, play: bool) -> Result<()> {
        let endpoint = if play { "/me/player/play" } else { "/me/player/pause" };
        self.request::<()>(reqwest::Method::PUT, endpoint, None).await
    }

    pub async fn next(&self) -> Result<()> {
        self.request::<()>(reqwest::Method::POST, "/me/player/next", None).await
    }

    pub async fn previous(&self) -> Result<()> {
        self.request::<()>(reqwest::Method::POST, "/me/player/previous", None).await
    }

    pub async fn seek(&self, position_ms: u64) -> Result<()> {
        let pos_str = position_ms.to_string();
        let params = [("position_ms", pos_str.as_str())];
        self.request::<()>(reqwest::Method::PUT, "/me/player/seek", Some(&params)).await
    }

    pub async fn set_volume(&self, volume: f64) -> Result<()> {
        let vol = (volume * 100.0).round() as u32;
        let vol_str = vol.to_string();
        let params = [("volume_percent", vol_str.as_str())];
        self.request::<()>(reqwest::Method::PUT, "/me/player/volume", Some(&params)).await
    }

    pub async fn set_shuffle(&self, shuffle: bool) -> Result<()> {
        let state_str = shuffle.to_string();
        let params = [("state", state_str.as_str())];
        self.request::<()>(reqwest::Method::PUT, "/me/player/shuffle", Some(&params)).await
    }

    pub async fn set_repeat(&self, mode: &str) -> Result<()> {
        let state = match mode {
            "Track" => "track",
            "Playlist" => "context",
            _ => "off",
        };
        let params = [("state", state)];
        self.request::<()>(reqwest::Method::PUT, "/me/player/repeat", Some(&params)).await
    }

    pub async fn get_user_profile(&self) -> Result<UserProfile> {
        self.request(reqwest::Method::GET, "/me", None).await
    }
}