//! Spotify Web API types

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

/// OAuth token response from Spotify
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct TokenResponse {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: i64,
    pub refresh_token: Option<String>,
    pub scope: String,
    #[serde(skip)]
    pub obtained_at: Option<DateTime<Utc>>,
}

impl TokenResponse {
    pub fn is_expired(&self) -> bool {
        if let Some(obtained) = self.obtained_at {
            let expires_at = obtained + chrono::Duration::seconds(self.expires_in - 60); // 60s buffer
            Utc::now() >= expires_at
        } else {
            true
        }
    }
}

/// Cached API response
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[allow(dead_code)]
pub struct CachedResponse {
    pub id: Uuid,
    pub endpoint: String,
    pub params_hash: String,
    pub response_json: String,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

/// Currently playing track state
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct PlaybackState {
    pub device: Option<Device>,
    pub repeat_state: String,
    pub shuffle_state: bool,
    pub context: Option<Context>,
    pub timestamp: i64,
    pub progress_ms: Option<i64>,
    pub is_playing: bool,
    pub item: Option<Track>,
    pub actions: PlaybackActions,
}

/// Device info
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct Device {
    pub id: String,
    pub is_active: bool,
    pub is_private_session: bool,
    pub is_restricted: bool,
    pub name: String,
    pub type_: String,
    pub volume_percent: Option<i32>,
    pub supports_volume: bool,
}

/// Playback context (album, playlist, etc.)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct Context {
    pub uri: String,
    pub href: String,
    pub external_urls: std::collections::HashMap<String, String>,
    pub type_: String,
}

/// Playback actions available
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct PlaybackActions {
    pub interrupting_playback: bool,
    pub pausing: bool,
    pub resuming: bool,
    pub seeking: bool,
    pub skipping_next: bool,
    pub skipping_prev: bool,
    pub toggling_repeat_context: bool,
    pub toggling_shuffle: bool,
    pub toggling_repeat_track: bool,
    pub transferring_playback: bool,
}

/// Track object
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct Track {
    pub id: String,
    pub name: String,
    pub artists: Vec<Artist>,
    pub album: Album,
    pub duration_ms: i32,
    pub explicit: bool,
    pub external_ids: std::collections::HashMap<String, String>,
    pub external_urls: std::collections::HashMap<String, String>,
    pub href: String,
    pub uri: String,
    pub preview_url: Option<String>,
    pub track_number: i32,
    pub disc_number: i32,
    pub is_playable: Option<bool>,
    pub popularity: i32,
}

/// Artist object
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct Artist {
    pub id: String,
    pub name: String,
    pub external_urls: std::collections::HashMap<String, String>,
    pub href: String,
    pub uri: String,
}

/// Album object
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct Album {
    pub id: String,
    pub name: String,
    pub artists: Vec<Artist>,
    pub images: Vec<Image>,
    pub external_urls: std::collections::HashMap<String, String>,
    pub href: String,
    pub uri: String,
    pub release_date: String,
    pub total_tracks: i32,
    pub album_type: String,
}

/// Image object
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct Image {
    pub url: String,
    pub height: Option<i32>,
    pub width: Option<i32>,
}

/// User profile
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserProfile {
    pub id: String,
    pub display_name: Option<String>,
    pub email: Option<String>,
    pub external_urls: std::collections::HashMap<String, String>,
    pub followers: Followers,
    pub href: String,
    pub images: Vec<Image>,
    pub product: String,
    pub uri: String,
}

/// Followers object
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Followers {
    pub href: Option<String>,
    pub total: i64,
}

/// Top tracks response
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct TopTracksResponse {
    pub items: Vec<Track>,
    pub total: i32,
    pub limit: i32,
    pub offset: i32,
    pub href: String,
    pub next: Option<String>,
    pub previous: Option<String>,
}

/// Recently played response
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct RecentlyPlayedResponse {
    pub items: Vec<PlayHistoryItem>,
    pub total: i32,
    pub limit: i32,
    pub offset: i32,
    pub href: String,
    pub next: Option<String>,
    pub previous: Option<String>,
}

/// Play history item
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct PlayHistoryItem {
    pub track: Track,
    pub played_at: DateTime<Utc>,
    pub context: Option<Context>,
}

/// Search response
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct SearchResponse {
    pub tracks: SearchResult<Track>,
    pub artists: SearchResult<Artist>,
    pub albums: SearchResult<Album>,
    pub playlists: SearchResult<Playlist>,
}

/// Generic search result
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct SearchResult<T> {
    pub items: Vec<T>,
    pub total: i32,
    pub limit: i32,
    pub offset: i32,
    pub href: String,
    pub next: Option<String>,
    pub previous: Option<String>,
}

/// Playlist object
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct Playlist {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub external_urls: std::collections::HashMap<String, String>,
    pub href: String,
    pub uri: String,
    pub images: Vec<Image>,
    pub owner: UserProfile,
    pub public: bool,
    pub collaborative: bool,
    pub tracks: PlaylistTracks,
}

/// Playlist tracks object
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct PlaylistTracks {
    pub href: String,
    pub total: i32,
}

/// Player state for plugin (simplified)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerState {
    pub status: String,           // Playing, Paused, Stopped
    pub title: String,
    pub artist: String,
    pub album: String,
    pub art_url: String,
    pub track_id: String,
    pub position: u64,            // microseconds
    pub duration: u64,            // microseconds
    pub volume: f64,              // 0.0 - 1.0
    pub shuffle: bool,
    pub loop_status: String,      // None, Track, Playlist
}