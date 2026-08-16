-- panel.luau
-- Spotify MPRIS - details panel (layered info-style UI, 400x520).
--
-- Layout: header + album art hero + track info + progress bar + controls.
-- Reads state from the service entry; sends commands via "sp.cmd".

local PLUGIN_ID = "roddygithub/spotify-mpris"

-- ── Command channel ────────────────────────────────────────────────────────────
local function sendCmd(cmd)
	cmd.seq = math.floor(os.clock() * 1000000)
	noctalia.state.set("sp.cmd", cmd)
end

-- ── Media slider drag state ──────────────────────────────────────────────────
local dragState = {}   -- dragging = true, position = <microseconds>

-- ── Helpers ────────────────────────────────────────────────────────────────────
local function fmtTime(us)
	if type(us) ~= "number" or us <= 0 then return "0:00" end
	local s = math.floor(us / 1000000)
	local m = math.floor(s / 60)
	s = s % 60
	return string.format("%d:%02d", m, s)
end

local function formatMetadata(metadata, key, default)
	if metadata and metadata[key] then
		local v = metadata[key]
		if type(v) == "table" and #v > 0 then return v[1] end
		if type(v) == "string" then return v end
	end
	return default or ""
end

-- ── Album art hero card ────────────────────────────────────────────────────────
local function heroCard(player)
	local showArt = noctalia.getConfig("show_album_art") == true
	local artUrl = player.artUrl or ""
	local playing = player.playbackState == "Playing"

	-- Verify art URL is accessible (basic check for file:// URLs)
	local hasArt = showArt and artUrl ~= ""
	if hasArt and artUrl:sub(1, 7) == "file://" then
		local path = artUrl:sub(8)
		if not noctalia.fileExists(path) then
			hasArt = false
		end
	end

	local title = player.trackTitle or "—"
	local artist = ""
	if player.trackArtists and #player.trackArtists > 0 then
		artist = player.trackArtists[1]
	end
	local album = player.trackAlbum or ""

	return ui.column({ gap = 12, padding = 16, align = "center" }, {
		hasArt and ui.image({
			path = artUrl,
			width = 200,
			height = 200,
			radius = 8,
			fit = "cover"
		}) or ui.box({ width = 200, height = 200, radius = 8, fill = "primary/0.1" }, {
			ui.glyph({ name = "music", size = 64, color = "primary/0.5" }),
		}),

		ui.column({ gap = 4, align = "center", width = 340 }, {
			ui.label({ text = title, fontSize = 18, fontWeight = "bold", color = "on_surface", maxLines = 1, elide = "end" }),
			ui.label({ text = artist, fontSize = 14, color = "on_surface_variant", maxLines = 1, elide = "end" }),
			album ~= "" and ui.label({ text = album, fontSize = 12, color = "on_surface/0.6", maxLines = 1, elide = "end" }) or nil,
		}),

		-- Playback status indicator
		ui.row({ gap = 8, align = "center" }, {
			ui.glyph({
				name = playing and "player-pause" or "player-play",
				size = 16,
				color = playing and "primary" or "on_surface_variant"
			}),
			ui.label({
				text = playing and "Playing" or (player.playbackState == "Paused" and "Paused" or "Stopped"),
				fontSize = 11,
				color = playing and "primary" or "on_surface_variant",
				fontWeight = "medium"
			}),
		}),
	})
end

-- ── Progress bar section ──────────────────────────────────────────────────────
local function progressSection(player)
	local canSeek = player.canSeek == true
	local length = tonumber(player.length) or 0
	local position = tonumber(player.position) or 0

	if not canSeek or length <= 0 then
		return ui.row({ gap = 8, align = "center" }, {
			ui.label({ text = fmtTime(position), fontSize = 11, color = "on_surface_variant" }),
			ui.label({ text = fmtTime(length), fontSize = 11, color = "on_surface_variant" }),
		})
	end

	local st = dragState[player.identity or "default"]
	local sliderValue = (st and st.dragging and st.position) or position

	return ui.column({ gap = 4, width = 360 }, {
		ui.slider({
			min = 0,
			max = length,
			step = 1000000, -- 1 second steps
			value = sliderValue,
			controlSize = "md",
			onChange = function(v)
				dragState[player.identity or "default"] = { dragging = true, position = tonumber(v) or position }
			end,
			onDragEnd = function()
				local st2 = dragState[player.identity or "default"]
				local target = (st2 and st2.position) or position
				dragState[player.identity or "default"] = nil
				sendCmd({ op = "seek", offset = tostring(target) })
			end
		}),
		ui.row({ justify = "space_between", width = 360 }, {
			ui.label({ text = fmtTime(sliderValue), fontSize = 10, color = "on_surface_variant" }),
			ui.label({ text = fmtTime(length), fontSize = 10, color = "on_surface_variant" }),
		}),
	})
end

-- ── Control buttons ────────────────────────────────────────────────────────────
local function controlsSection(player)
	local playing = player.playbackState == "Playing"
	local canControl = player.canControl == true
	local canGoNext = player.canGoNext == true
	local canGoPrev = player.canGoPrevious == true
	local shuffle = player.shuffle == true
	local loopStatus = player.loopStatus or "None"

	local function actionBtn(glyph, onClick, variant, enabled, tooltip)
		if enabled == false then
			return ui.button({
				glyph = glyph,
				variant = "ghost",
				controlSize = "lg",
				enabled = false,
				tooltip = tooltip
			})
		end
		return ui.button({
			glyph = glyph,
			variant = variant or "outline",
			controlSize = "lg",
			onClick = onClick,
			tooltip = tooltip
		})
	end

	return ui.row({ gap = 16, align = "center", justify = "center" }, {
		-- Shuffle
		actionBtn(
			shuffle and "shuffle-on" or "shuffle",
			function() sendCmd({ op = "shuffle", value = not shuffle }) end,
			shuffle and "primary" or "outline",
			canControl,
			shuffle and "Disable shuffle" or "Enable shuffle"
		),
		-- Previous
		actionBtn(
			"player-skip-back",
			function() sendCmd({ op = "previous" }) end,
			"outline",
			canGoPrev,
			"Previous track"
		),
		-- Play/Pause
		actionBtn(
			playing and "player-pause" or "player-play",
			function() sendCmd({ op = "play_pause" }) end,
			"primary",
			canControl,
			playing and "Pause" or "Play"
		),
		-- Next
		actionBtn(
			"player-skip-forward",
			function() sendCmd({ op = "next" }) end,
			"outline",
			canGoNext,
			"Next track"
		),
		-- Loop
		actionBtn(
			loopStatus == "Track" and "repeat-one" or (loopStatus == "Playlist" and "repeat" or "repeat-off"),
			function() sendCmd({ op = "loop" }) end,
			(loopStatus ~= "None") and "primary" or "outline",
			canControl,
			"Cycle repeat mode"
		),
	})
end

-- ── Volume slider ──────────────────────────────────────────────────────────────
local function volumeSection(player)
	local volume = tonumber(player.volume) or 1.0
	local canControl = player.canControl == true

	return ui.column({ gap = 6, width = 360, align = "center" }, {
		ui.row({ gap = 8, align = "center" }, {
			ui.glyph({ name = volume > 0.5 and "volume-high" or (volume > 0 and "volume-low" or "volume-off"), size = 16, color = "on_surface_variant" }),
			ui.slider({
				min = 0,
				max = 100,
				step = 5,
				value = math.floor(volume * 100),
				controlSize = "sm",
				enabled = canControl,
				width = 200,
				onChange = function(v)
					sendCmd({ op = "volume", volume = tostring((tonumber(v) or 0) / 100) })
				end,
			}),
			ui.label({ text = tostring(math.floor(volume * 100)) .. "%", fontSize = 11, color = "on_surface_variant", width = 35 }),
		}),
	})
end

-- ── Additional info grid ───────────────────────────────────────────────────────
local function infoGrid(player)
	local metadata = player.metadata or {}
	local items = {}

	-- Track ID (shortened)
	if metadata["mpris:trackid"] then
		local id = metadata["mpris:trackid"]
		if #id > 40 then id = id:sub(1, 37) .. "..." end
		table.insert(items, { label = "Track ID", value = id, glyph = "fingerprint" })
	end

	-- Bitrate / format info if available
	if metadata["xesam:contentCreated"] then
		table.insert(items, { label = "Created", value = metadata["xesam:contentCreated"], glyph = "calendar" })
	end

	if #items == 0 then return nil end

	local rows = {}
	for _, item in ipairs(items) do
		table.insert(rows, ui.column({ gap = 2, padding = 8, radius = 6, fill = "surface/0.5", flexGrow = 1 }, {
			ui.row({ gap = 6, align = "center" }, {
				ui.glyph({ name = item.glyph, size = 12, color = "on_surface_variant" }),
				ui.label({ text = item.label, fontSize = 9, color = "on_surface_variant" }),
			}),
			ui.label({ text = item.value, fontSize = 11, fontWeight = "medium", color = "on_surface", maxLines = 1, elide = "middle" }),
		}))
	end

	return ui.row({ gap = 8 }, rows)
end

-- ── Main render ────────────────────────────────────────────────────────────────
function render()
	local backend = noctalia.state.get("sp.backend") or { available = false }
	local player = noctalia.state.get("sp.player") or {}

	local body = {}

	-- Header
	table.insert(body, ui.row({ gap = 8, align = "center" }, {
		ui.glyph({ name = "music", size = 18, color = "primary" }),
		ui.label({ text = "Spotify MPRIS", fontSize = 16, fontWeight = "bold", color = "on_surface", flexGrow = 1 }),
		ui.button({ glyph = "refresh", variant = "ghost", controlSize = "sm", onClick = function() sendCmd({ op = "refresh" }) end, tooltip = "Refresh" }),
		ui.button({ glyph = "close", variant = "ghost", controlSize = "sm", onClick = function() panel.close() end, tooltip = "Close" }),
	}))

	if not backend.available then
		table.insert(body, ui.column({ gap = 8, padding = 40, align = "center", flexGrow = 1 }, {
			ui.glyph({ name = "music-off", size = 40, color = "on_surface_variant" }),
			ui.label({ text = "No MPRIS player available", color = "on_surface_variant" }),
			ui.label({ text = "Start Spotify or another MPRIS player", fontSize = 11, color = "on_surface/0.5" }),
		}))
	elseif not player.identity then
		table.insert(body, ui.column({ gap = 8, padding = 40, align = "center", flexGrow = 1 }, {
			ui.glyph({ name = "music-off", size = 40, color = "on_surface_variant" }),
			ui.label({ text = "Nothing playing", color = "on_surface_variant" }),
			ui.label({ text = "Start playback in Spotify", fontSize = 11, color = "on_surface/0.5" }),
		}))
	else
		-- Hero with album art
		table.insert(body, heroCard(player))

		-- Progress bar
		table.insert(body, progressSection(player))

		-- Main controls
		table.insert(body, controlsSection(player))

		-- Volume
		table.insert(body, volumeSection(player))

		-- Additional info
		local info = infoGrid(player)
		if info then table.insert(body, info) end
	end

	panel.render(ui.column({ gap = 12, padding = 16, width = 380 }, body))
end

function onOpen(_context) render() end
function onClose() end

noctalia.state.watch("sp.player", render)
noctalia.state.watch("sp.backend", render)
noctalia.state.watch("sp.trTable", render)