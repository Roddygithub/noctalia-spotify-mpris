-- Initial schema for noctalia-spotify-backend

-- OAuth tokens table
CREATE TABLE IF NOT EXISTS oauth_tokens (
    key TEXT PRIMARY KEY,
    access_token TEXT NOT NULL,
    token_type TEXT NOT NULL,
    expires_in INTEGER NOT NULL,
    refresh_token TEXT,
    scope TEXT NOT NULL,
    obtained_at TEXT NOT NULL
);

-- API response cache table
CREATE TABLE IF NOT EXISTS api_cache (
    id TEXT PRIMARY KEY,
    endpoint TEXT NOT NULL,
    params_hash TEXT NOT NULL,
    response_json TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE(endpoint, params_hash)
);

-- Index for cache cleanup
CREATE INDEX IF NOT EXISTS idx_api_cache_expires_at ON api_cache(expires_at);

-- User settings table
CREATE TABLE IF NOT EXISTS user_settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- Insert default settings
INSERT OR IGNORE INTO user_settings (key, value) VALUES
    ('device_name', 'Noctalia Spotify'),
    ('bitrate', '320'),
    ('volume_normalization', 'true');