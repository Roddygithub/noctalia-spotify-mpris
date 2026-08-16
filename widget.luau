-- widget.luau
-- Spotify MPRIS - bar widget (thin UI entry).
--
-- Renders a music glyph with optional playback state indication.
-- Left-click opens the details panel; right-click opens settings.
-- Middle-click toggles play/pause.
-- Reads state from the service entry; no MPRIS logic here.

local PLUGIN_ID = "roddygithub/spotify-mpris"

-- Local translation (if we add translations later)
local function t(key)
	local table = noctalia.state.get("sp.trTable") or {}
	return table[key] or key
end

local function panelId()
	return PLUGIN_ID .. ":details"
end

local function render()
	local backend = noctalia.state.get("sp.backend") or { available = false }
	local player = noctalia.state.get("sp.player") or {}
	local showArtist = noctalia.getConfig("show_artist") == true

	local children
	if not backend.available then
		children = {
			ui.glyph({ name = "music-off", size = 14, color = "on_surface/0.4" }),
			ui.label({ text = "No player", fontSize = 10, color = "on_surface/0.5" }),
		}
	elseif not player.identity then
		children = {
			ui.glyph({ name = "music-off", size = 14, color = "on_surface/0.4" }),
			ui.label({ text = "Not playing", fontSize = 10, color = "on_surface/0.5" }),
		}
	else
		local playing = player.playbackState == "Playing"
		local glyphColor = playing and "primary" or "on_surface"

		local kids = { ui.glyph({ name = "music", size = 14, color = glyphColor }) }

		if showArtist and player.trackArtists and #player.trackArtists > 0 then
			local artist = player.trackArtists[1]
			table.insert(kids, ui.label({ text = artist, fontSize = 11, color = glyphColor }))
		elseif player.trackTitle and player.trackTitle ~= "" then
			table.insert(kids, ui.label({ text = player.trackTitle, fontSize = 11, color = glyphColor, maxWidth = 120 }))
		end

		children = kids
	end

	local container = barWidget.isVertical() and ui.column or ui.row
	barWidget.render(container({ gap = 5, align = "center" }, children))
end

function update()
	local interval = tonumber(noctalia.getConfig("update_interval")) or 1000
	noctalia.setUpdateInterval(interval)
	render()
end

function onClick()
	noctalia.togglePanel(panelId())
end

function onMiddleClick()
	local player = noctalia.state.get("sp.player") or {}
	if player.identity then
		sendCmd({ op = "play_pause" })
	end
end

function onRightClick()
	noctalia.openSettings()
end

local function sendCmd(cmd)
	cmd.seq = math.floor(os.clock() * 1000000)
	noctalia.state.set("sp.cmd", cmd)
end

noctalia.state.watch("sp.player", render)
noctalia.state.watch("sp.backend", render)
noctalia.state.watch("sp.trTable", render)

render()