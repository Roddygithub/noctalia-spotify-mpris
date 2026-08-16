-- service.luau
-- Spotify MPRIS - background service that polls MPRIS via playerctl.
--
-- Architecture: Noctalia plugin scripts run in an isolated sandbox with NO
-- `require`/`dofile`/`loadfile`. All MPRIS interaction lives here and
-- is published to UI entries (widget/panel) as plain data via `noctalia.state`.

local PLUGIN_ID = "roddygithub/spotify-mpris"

-- ── State contract (read by UI entries) ───────────────────────────────────────
--   noctalia.state "sp.backend"   : { available=bool, name=string }
--   noctalia.state "sp.player"    : { identity, name, playbackState, loopStatus, canGoNext, canGoPrevious,
--                                     canControl, canSeek, volume, position, length,
--                                     trackTitle, trackArtists, trackAlbum, artUrl,
--                                     shuffle, metadata }
--   noctalia.state "sp.cmd"       : { op=string, [args...] }
--                                   written by UI entries; service executes and clears.
--                                   ops: play_pause, next, previous, seek, volume, shuffle, loop

-- ── Helpers ────────────────────────────────────────────────────────────────────
local function shellQuote(s)
	return "'" .. tostring(s):gsub("'", "'\\''") .. "'"
end

local function parsePlayerctlJson(raw)
	if not raw or raw == "" then return nil end
	local ok, data = pcall(noctalia.json.decode, raw)
	if ok and type(data) == "table" then return data end
	return nil
end

-- ── Player state ──────────────────────────────────────────────────────────────
local playerState = {}
local lastPlayerIdentity = nil

local function publishPlayer()
	noctalia.state.set("sp.player", playerState)
end

local function publishBackend(available, name)
	noctalia.state.set("sp.backend", { available = available, name = name or "None" })
end

-- ── Fetch player metadata via playerctl ────────────────────────────────────────
local function fetchPlayers()
	local filter = noctalia.getConfig("filter_player") or "spotify"
	local cmd
	if filter == "spotify" then
		cmd = "playerctl -l 2>/dev/null | grep -i spotify | head -1"
	else
		cmd = "playerctl -l 2>/dev/null | head -1"
	end

	noctalia.runAsync(cmd, function(res)
		if not res or res.error or not res.stdout or res.stdout == "" then
			-- No player found
			if next(playerState) ~= nil then
				playerState = {}
				publishPlayer()
			end
			publishBackend(false, "No MPRIS player")
			return
		end

		local playerName = res.stdout:match("^%s*(.-)%s*$")
		if playerName == lastPlayerIdentity and next(playerState) ~= nil then
			-- Same player, just refresh metadata
			fetchMetadata(playerName)
			return
		end

		lastPlayerIdentity = playerName
		fetchMetadata(playerName)
	end)
end

local function fetchMetadata(playerName)
	-- Get all metadata at once using playerctl metadata --format json
	local cmd = "playerctl -p " .. shellQuote(playerName) .. " metadata --format json 2>/dev/null"
	noctalia.runAsync(cmd, function(res)
		if not res or res.error then
			playerState = { identity = playerName, name = playerName, error = res.error or "metadata failed" }
			publishPlayer()
			publishBackend(true, playerName)
			return
		end

		local metadata = parsePlayerctlJson(res.stdout)
		if not metadata then
			playerState = { identity = playerName, name = playerName, error = "parse failed" }
			publishPlayer()
			publishBackend(true, playerName)
			return
		end

		-- Also get playback status, volume, position, length, shuffle, loop
		fetchPlaybackInfo(playerName, metadata)
	end)
end

local function fetchPlaybackInfo(playerName, metadata)
	local cmds = {
		status = "playerctl -p " .. shellQuote(playerName) .. " status 2>/dev/null",
		volume = "playerctl -p " .. shellQuote(playerName) .. " volume 2>/dev/null",
		position = "playerctl -p " .. shellQuote(playerName) .. " position 2>/dev/null",
		length = "playerctl -p " .. shellQuote(playerName) .. " metadata --format '{{mpris:length}}' 2>/dev/null",
		shuffle = "playerctl -p " .. shellQuote(playerName) .. " shuffle 2>/dev/null",
		loop = "playerctl -p " .. shellQuote(playerName) .. " loop 2>/dev/null",
		can_control = "playerctl -p " .. shellQuote(playerName) .. " metadata --format '{{mpris:canControl}}' 2>/dev/null",
		can_seek = "playerctl -p " .. shellQuote(playerName) .. " metadata --format '{{mpris:canSeek}}' 2>/dev/null",
		can_go_next = "playerctl -p " .. shellQuote(playerName) .. " metadata --format '{{mpris:canGoNext}}' 2>/dev/null",
		can_go_prev = "playerctl -p " .. shellQuote(playerName) .. " metadata --format '{{mpris:canGoPrevious}}' 2>/dev/null",
	}

	local results = {}
	local pending = 0
	for k, v in pairs(cmds) do
		pending = pending + 1
		noctalia.runAsync(v, function(res)
			if res and not res.error and res.stdout then
				results[k] = res.stdout:match("^%s*(.-)%s*$")
			else
				results[k] = nil
			end
			pending = pending - 1
			if pending == 0 then
				buildPlayerState(playerName, metadata, results)
			end
		end)
	end
end

local function buildPlayerState(playerName, metadata, info)
	local function parseBool(v)
		if not v then return false end
		v = v:lower()
		return v == "true" or v == "yes" or v == "on" or v == "1"
	end

	local function parseFloat(v)
		if not v then return 0 end
		local n = tonumber(v)
		return n or 0
	end

	local function parseInt(v)
		if not v then return 0 end
		local n = tonumber(v)
		return math.floor(n or 0)
	end

	-- Extract track info from metadata
	local title = metadata["xesam:title"] or ""
	local artists = metadata["xesam:artist"] or {}
	local album = metadata["xesam:album"] or ""
	local artUrl = metadata["mpris:artUrl"] or ""
	local trackId = metadata["mpris:trackid"] or ""

	-- Normalize artists to array
	if type(artists) == "string" then artists = { artists } end

	-- Playback state
	local status = info.status or "Stopped"
	local playbackState
	if status == "Playing" then playbackState = "Playing"
	elseif status == "Paused" then playbackState = "Paused"
	else playbackState = "Stopped" end

	playerState = {
		identity = playerName,
		name = playerName,
		playbackState = playbackState,
		loopStatus = info.loop or "None",
		canGoNext = parseBool(info.can_go_next),
		canGoPrevious = parseBool(info.can_go_prev),
		canControl = parseBool(info.can_control),
		canSeek = parseBool(info.can_seek),
		volume = parseFloat(info.volume),
		position = parseInt(info.position), -- microseconds
		length = parseInt(info.length), -- microseconds
		trackTitle = title,
		trackArtists = artists,
		trackAlbum = album,
		artUrl = artUrl,
		trackId = trackId,
		shuffle = parseBool(info.shuffle),
		metadata = metadata,
	}

	publishPlayer()
	publishBackend(true, playerName)
end

-- ── Command execution (from UI entries via sp.cmd) ─────────────────────────────
local function handleCommand(cmd)
	if type(cmd) ~= "table" then return end
	local op = cmd.op
	local player = playerState.identity

	if not player then return end

	local function runPlayerctl(args, cb)
		local fullCmd = "playerctl -p " .. shellQuote(player) .. " " .. args
		noctalia.runAsync(fullCmd, function(res)
			if res and res.error then
				noctalia.log("[spotify-mpris] playerctl failed: " .. res.error)
			end
			-- Refresh after any mutating command
			noctalia.runAsync("sleep 0.1", function() fetchPlayers() end)
			if cb then cb(res) end
		end)
	end

	if op == "play_pause" then
		runPlayerctl("play-pause")
	elseif op == "next" then
		runPlayerctl("next")
	elseif op == "previous" then
		runPlayerctl("previous")
	elseif op == "seek" then
		local offset = tonumber(cmd.offset) or 0 -- microseconds
		runPlayerctl("position " .. tostring(offset))
	elseif op == "volume" then
		local vol = tonumber(cmd.volume) or 0 -- 0.0-1.0
		runPlayerctl("volume " .. tostring(vol))
	elseif op == "shuffle" then
		local val = cmd.value
		if val == nil then val = not playerState.shuffle end
		runPlayerctl("shuffle " .. (val and "On" or "Off"))
	elseif op == "loop" then
		local val = cmd.value
		if val == nil then
			local current = playerState.loopStatus or "None"
			if current == "None" then val = "Track"
			elseif current == "Track" then val = "Playlist"
			else val = "None" end
		end
		runPlayerctl("loop " .. val)
	elseif op == "refresh" then
		fetchPlayers()
	else
		noctalia.log("[spotify-mpris] unknown cmd op: " .. tostring(op))
	end
end

-- ── Lifecycle ──────────────────────────────────────────────────────────────────
local function intervalMs()
	local ms = tonumber(noctalia.getConfig("update_interval")) or 1000
	if ms < 500 then return 500 end
	if ms > 5000 then return 5000 end
	return ms
end

function update()
	noctalia.setUpdateInterval(intervalMs())
	fetchPlayers()
end

function onConfigChanged()
	noctalia.setUpdateInterval(intervalMs())
	fetchPlayers()
end

function onIpc(event, payload)
	if event == "refresh" then
		fetchPlayers()
	elseif event == "cmd" and payload then
		local ok, cmd = pcall(noctalia.json.decode, payload)
		if ok and type(cmd) == "table" then
			handleCommand(cmd)
		else
			noctalia.log("[spotify-mpris] onIpc cmd: invalid json payload")
		end
	end
end

-- Initial load
fetchPlayers()

-- Command channel: UI entries write { op, seq, ... } to "sp.cmd".
-- seq is a high-resolution timestamp so it never resets on hot-reload.
local lastCmdSeq = 0
noctalia.state.watch("sp.cmd", function(cmd)
	if type(cmd) ~= "table" then return end
	local seq = tonumber(cmd.seq) or 0
	if seq <= lastCmdSeq then return end
	lastCmdSeq = seq
	cmd.seq = nil
	handleCommand(cmd)
end)