# Spotify MPRIS Plugin for Noctalia v5

A Noctalia v5 plugin that adds a Spotify controller to your bar with a detailed popup panel — album art, seekable progress bar, playback controls, volume, shuffle, and loop.

Supports two backends:
- **Native** (librespot) — full Spotify Connect device, no Spotify client needed
- **MPRIS** — controls any MPRIS-compatible player (Spotify, Spot, etc.)

## Features

- **Bar widget**: Shows current track artist/title with play indicator + backend badge (● Native / ● MPRIS)
- **Popup panel** (click widget): 
  - Album art (200×200, cropped)
  - Track title, artist, album
  - Seekable progress bar with time labels
  - Play/Pause, Previous, Next buttons
  - Volume slider
  - Shuffle toggle
  - Loop mode cycle (Off → Track → Playlist)
  - Backend indicator + Authenticate button (native mode)
- **Middle-click** on widget: quick play/pause toggle
- **Right-click** on widget: open settings
- **Keyboard shortcuts** in panel: Space/Enter=play/pause, ←/→=prev/next, Esc=close

## Requirements

### MPRIS Backend (default)
- **Noctalia v5** (Hyprland-based)
- **playerctl** — MPRIS CLI controller (`sudo pacman -S playerctl`)
- **Spotify** (or any MPRIS-compatible player) running

### Native Backend (librespot)
- **Noctalia v5** (Hyprland-based)
- **noctalia-spotify-backend** — Rust daemon (built from source, provides librespot + Web API)
- Systemd user service for background daemon

## Installation

### Via Noctalia Plugin Manager (recommended)

```bash
# Add the plugin source
noctalia msg plugins source add spotify-mpris git https://github.com/Roddygithub/noctalia-spotify-mpris

# Update sources
noctalia msg plugins update spotify-mpris

# Enable the plugin
noctalia msg plugins enable roddygithub/spotify-mpris

# Add widget to bar via Settings UI or:
noctalia msg settings-open
```

### Manual Installation

```bash
# Clone to Noctalia local plugins directory
git clone https://github.com/Roddygithub/noctalia-spotify-mpris.git \
  ~/.local/share/noctalia/plugins/spotify-mpris

# Update local source
noctalia msg plugins update local

# Enable
noctalia msg plugins enable roddygithub/spotify-mpris
```

### Native Backend Setup

```bash
# Build the backend (requires Rust toolchain)
cd /path/to/noctalia-spotify-mpris/backend  # or separate repo
cargo build --release

# Install systemd user unit (provided in backend repo)
systemctl --user enable --now noctalia-spotify-backend

# Verify socket exists
ls $XDG_RUNTIME_DIR/noctalia-spotify/backend.sock
```

### Spotify Client ID (required for native mode OAuth)

The native backend uses OAuth to access your Spotify account. You need a Spotify app:

1. Create an app at <https://developer.spotify.com/dashboard>
2. Add `http://127.0.0.1:8000/callback` as a **Redirect URI** (note: Spotify no longer accepts `http://localhost:...` — the loopback IP literal `127.0.0.1` is required)
3. Configure the client ID in one of two ways:

**Option A — config file** (`~/.config/noctalia-spotify-backend/config.toml`):
```toml
client_id = "your-app-client-id"
```

**Option B — environment variable** (in `~/.config/systemd/user/noctalia-spotify-backend.service`):
```
[Service]
Environment=SPOTIFY_CLIENT_ID=your-app-client-id
```

Then restart the backend:
```bash
systemctl --user restart noctalia-spotify-backend
```

Click the **Authenticate** button in the plugin panel (or open `http://localhost:8000/login`), approve in your browser, and the token is cached locally.

## Configuration

Open settings via right-click on widget or:
```bash
noctalia msg settings-open-plugin roddygithub/spotify-mpris
```

| Setting | Type | Default | Description |
|---------|------|---------|-------------|
| `update_interval` | int (ms) | 1000 | Polling interval for MPRIS updates |
| `show_artist` | bool | true | Show artist name in bar widget |
| `show_album_art` | bool | true | Show album art in panel |
| `backend_mode` | select | auto | `auto` = prefer native, fallback MPRIS; `native` = librespot only; `mpris` = MPRIS only |
| `filter_player` | select | any | `any` = first MPRIS player; `spotify` = Spotify only (MPRIS mode) |
| `language` | select | en | `en` = English; `fr` = Français |

### Backend Modes

| Mode | Behavior |
|------|----------|
| `auto` (default) | Tries native backend first (Unix socket ping), falls back to MPRIS if unavailable |
| `native` | Uses `noctalia-spotify-backend` via Unix socket at `$XDG_RUNTIME_DIR/noctalia-spotify/backend.sock`. Shows "Authenticate" button in panel for OAuth flow. |
| `mpris` | Uses system D-Bus via `gdbus` to control any MPRIS player (Spotify, Spot, etc.) |

## Architecture

```
┌─────────────────┐     noctalia.state      ┌─────────────┐
│  service.luau   │ ──────────────────────▶ │ widget.luau │
│  (background)   │   sp.player, sp.backend │  (bar)      │
│  - gdbus poll   │                         └──────┬──────┘
│  - native sock  │                                │
└─────────────────┘                                ▼
                           ┌─────────────────────────────────┐
                           │       panel.luau (popup)        │
                           │  - hero album art               │
                           │  - progress bar (drag to seek)  │
                           │  - transport controls           │
                           │  - volume, shuffle, loop        │
                           │  - backend badge + Auth btn     │
                           └─────────────────────────────────┘
```

- **service.luau**: Background service polling MPRIS via `gdbus` or Unix socket (native), publishes player state to `noctalia.state`
- **widget.luau**: Bar widget reading state, renders compact track info + backend indicator
- **panel.luau**: Detailed popup with full controls, backend badge, authenticate button

## Commands

| Action | Command |
|--------|---------|
| Toggle panel | `noctalia msg panel-toggle roddygithub/spotify-mpris:details` |
| Refresh player | `noctalia msg plugin roddygithub/spotify-mpris:service bar refresh` |
| Open settings | `noctalia msg settings-open-plugin roddygithub/spotify-mpris` |

## IPC Commands (via `sp.cmd` state)

| Command | Args | Description |
|---------|------|-------------|
| `play_pause` | — | Toggle playback |
| `next` | — | Next track |
| `previous` | — | Previous track |
| `seek` | `offset` (µs) | Seek to position |
| `volume` | `volume` (0.0–1.0) | Set volume |
| `shuffle` | `value` (bool) | Toggle shuffle |
| `loop` | `value` (string) | Cycle: None → Track → Playlist |
| `authenticate` | — | Start OAuth flow (native only) |
| `refresh` | — | Re-discover player |

## Global Keybind Example

Add to your Hyprland/Noctalia keybinds:
```lua
-- Toggle Spotify panel with SUPER+M
{ mod = "SUPER", key = "M", action = "panel:roddygithub/spotify-mpris:details:toggle" }
```

## Troubleshooting

**Widget shows "No player" / "Not playing"**
- Ensure Spotify is running and playing (MPRIS mode)
- For native mode: check `systemctl --user status noctalia-spotify-backend`
- Install `playerctl`: `sudo pacman -S playerctl` (MPRIS mode)
- Check `playerctl -l` lists a player

**Panel opens but empty**
- Service may still be polling (wait ~2s)
- Check `playerctl metadata` works in terminal (MPRIS mode)
- Check socket: `ls $XDG_RUNTIME_DIR/noctalia-spotify/backend.sock` (native mode)

**Album art not loading**
- Some MPRIS players return `file://` URLs requiring local file access
- Ensure the art URL is accessible (Spotify typically works)
- Native backend downloads art via backend

**Native backend: "Authenticate" button does nothing**
- Check backend logs: `journalctl --user -u noctalia-spotify-backend -f`
- Backend serves OAuth on `http://localhost:8000` — open in browser
- Ensure a Spotify `client_id` is configured (see "Spotify Client ID" above)

## License

MIT — see [LICENSE](LICENSE)

## Credits

Inspired by [Omaspotify](https://github.com/archlatam/omaspotify) for Omarchy/Quickshell. Built for Noctalia v5 plugin API.