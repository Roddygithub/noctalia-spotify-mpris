# Spotify MPRIS Plugin for Noctalia v5

A Noctalia v5 plugin that adds a Spotify controller to your bar with a detailed popup panel — album art, seekable progress bar, playback controls, volume, shuffle, and loop.

## Features

- **Bar widget**: Shows current track artist/title with play indicator
- **Popup panel** (click widget): 
  - Album art (200×200, cropped)
  - Track title, artist, album
  - Seekable progress bar with time labels
  - Play/Pause, Previous, Next buttons
  - Volume slider
  - Shuffle toggle
  - Loop mode cycle (Off → Track → Playlist)
- **Middle-click** on widget: quick play/pause toggle
- **Right-click** on widget: open settings
- **Keyboard shortcuts** in panel: Space/Enter=play/pause, ←/→=prev/next, Esc=close

## Requirements

- **Noctalia v5** (Hyprland-based)
- **playerctl** — MPRIS CLI controller (`sudo pacman -S playerctl`)
- **Spotify** (or any MPRIS-compatible player) running

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
| `filter_player` | select | spotify | `spotify` = only Spotify, `any` = first MPRIS player |

## Architecture

```
┌─────────────────┐     noctalia.state      ┌─────────────┐
│  service.luau   │ ──────────────────────▶ │ widget.luau │
│  (background)   │   sp.player, sp.backend │  (bar)      │
│  - playerctl    │                         └──────┬──────┘
│  - polls 1s     │                                │
└─────────────────┘                                ▼
                          ┌─────────────────────────────────┐
                          │       panel.luau (popup)        │
                          │  - hero album art               │
                          │  - progress bar (drag to seek)  │
                          │  - transport controls           │
                          │  - volume, shuffle, loop        │
                          └─────────────────────────────────┘
```

- **service.luau**: Background service polling `playerctl` every second, publishes player state to `noctalia.state`
- **widget.luau**: Bar widget reading state, renders compact track info
- **panel.luau**: Detailed popup with full controls

## Commands

| Action | Command |
|--------|---------|
| Toggle panel | `noctalia msg panel-toggle roddygithub/spotify-mpris:details` |
| Refresh player | `noctalia msg plugin roddygithub/spotify-mpris:service bar refresh` |
| Open settings | `noctalia msg settings-open-plugin roddygithub/spotify-mpris` |

## Global Keybind Example

Add to your Hyprland/Noctalia keybinds:
```lua
-- Toggle Spotify panel with SUPER+M
{ mod = "SUPER", key = "M", action = "panel:roddygithub/spotify-mpris:details:toggle" }
```

## Troubleshooting

**Widget shows "No player" / "Not playing"**
- Ensure Spotify is running and playing
- Install `playerctl`: `sudo pacman -S playerctl`
- Check `playerctl -l` lists a player

**Panel opens but empty**
- Service may still be polling (wait ~2s)
- Check `playerctl metadata` works in terminal

**Album art not loading**
- Some MPRIS players return `file://` URLs requiring local file access
- Ensure the art URL is accessible (Spotify typically works)

## License

MIT — see [LICENSE](LICENSE)

## Credits

Inspired by [Omaspotify](https://github.com/archlatam/omaspotify) for Omarchy/Quickshell. Built for Noctalia v5 plugin API.