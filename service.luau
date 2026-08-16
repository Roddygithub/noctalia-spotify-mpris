-- service.luau
-- Spotify MPRIS - background service that polls MPRIS via gdbus (no playerctl dependency).
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

local function parseGdbusValue(raw)
	if not raw then return nil end
	raw = raw:gsub("^%s+", ""):gsub("%s+$", "")
	-- variant form: (<value>,)
	local inner = raw:match("^%(<(.+)>%,?%)?$")
	if inner then
		if inner == "true" then return true end
		if inner == "false" then return false end
		local n = tonumber(inner)
		if n then return n end
		local s = inner:match("^'(.*)'$") or inner:match('^"(.*)"$')
		if s then return s end
		return inner
	end
	-- bare form: (true,) (false,) (-1,)
	local bare = raw:match("^%((.-)%,?%)$")
	if bare == nil then bare = raw end
	bare = bare:gsub("%s+$", ""):gsub(",%s*$", "")
	if bare == "true" then return true end
	if bare == "false" then return false end
	local n = tonumber(bare)
	if n then return n end
	local s = bare:match("^'(.*)'$") or bare:match('^"(.*)"$')
	if s then return s end
	return bare
end

local function parseGdbusStringArray(raw)
	if not raw then return {} end
	local arr = raw:match("%[(.-)%]")
	if not arr then return {} end
	local out = {}
	for item in arr:gmatch("'([^']*)'") do
		table.insert(out, item)
	end
	return out
end

local function parseGdbusDict(raw)
	-- Parse a{sv} dict like: ({'key': <'value'>, ...},)
	if not raw then return {} end
	local out = {}
	-- Simple key-value extraction
	for k, v in raw:gmatch("'([^']+)'%s*:%s*<([^>]+)>") do
		out[k] = parseGdbusValue("<" .. v .. ">")
	end
	return out
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

-- ── Discover Spotify player via D-Bus ─────────────────────────────────────────
local function discoverPlayer()
	local filter = noctalia.getConfig("filter_player") or "spotify"
	local cmd
	if filter == "spotify" then
		cmd = "gdbus call --session --dest org.freedesktop.DBus --object-path /org/freedesktop/DBus --method org.freedesktop.DBus.ListNames | grep -o 'org.mpris.MediaPlayer2.spotify[^\"\\]]*'"
	else
		cmd = "gdbus call --session --dest org.freedesktop.DBus --object-path /org/freedesktop/DBus --method org.freedesktop.DBus.ListNames | grep -o 'org.mpris.MediaPlayer2[^\"\\]]*' | head -1"
	end

	noctalia.runAsync(cmd, function(res)
		if not res or res.error or not res.stdout or res.stdout == "" then
			if next(playerState) ~= nil then
				playerState = {}
				publishPlayer()
			end
			publishBackend(false, "No MPRIS player")
			return
		end

		local busName = res.stdout:match("^%s*(.-)%s*$")
		if busName == lastPlayerIdentity and next(playerState) ~= nil then
			fetchMetadata(busName)
			return
		end

		lastPlayerIdentity = busName
		fetchMetadata(busName)
	end)
end

-- ── Fetch all metadata via gdbus ──────────────────────────────────────────────
local function fetchMetadata(busName)
	-- Get the player object path
	local objPath = "/org/mpris/MediaPlayer2"

	-- GetAll on Player interface
	local cmd = "gdbus call --session --dest " .. shellQuote(busName) ..
		" --object-path " .. shellQuote(objPath) ..
		" --method org.freedesktop.DBus.Properties.GetAll " ..
		shellQuote("org.mpris.MediaPlayer2.Player")

	noctalia.runAsync(cmd, function(res)
		if not res or res.error then
			playerState = { identity = busName, name = busName, error = res.error or "GetAll failed" }
			publishPlayer()
			publishBackend(true, busName)
			return
		end

		local props = parseGdbusDict(res.stdout or "")
		if not props or not next(props) then
			playerState = { identity = busName, name = busName, error = "parse failed" }
			publishPlayer()
			publishBackend(true, busName)
			return
		end

		-- Also get Metadata from Properties interface
		fetchMetadataDetails(busName, props)
	end)
end

local function fetchMetadataDetails(busName, playerProps)
	-- Get Metadata property separately (it's a dict)
	local cmd = "gdbus call --session --dest " .. shellQuote(busName) ..
		" --object-path " .. shellQuote("/org/mpris/MediaPlayer2") ..
		" --method org.freedesktop.DBus.Properties.Get " ..
		shellQuote("org.mpris.MediaPlayer2.Player") .. " " ..
		shellQuote("Metadata")

	noctalia.runAsync(cmd, function(res)
		local metadata = {}
		if res and not res.error then
			metadata = parseGdbusDict(res.stdout or "")
		end

		buildPlayerState(busName, metadata, playerProps)
	end)
end

local function buildPlayerState(busName, metadata, playerProps)
	local function parseBool(v)
		if v == nil then return false end
		if type(v) == "boolean" then return v end
		if type(v) == "number" then return v ~= 0 end
		local s = tostring(v):lower()
		return s == "true" or s == "yes" or s == "on" or s == "1"
	end

	local function parseFloat(v)
		if v == nil then return 0 end
		if type(v) == "number" then return v end
		local n = tonumber(v)
		return n or 0
	end

	local function parseInt(v)
		if v == nil then return 0 end
		if type(v) == "number" then return math.floor(v) end
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

	-- Playback state from player props
	local status = playerProps["PlaybackStatus"] or "Stopped"
	local playbackState
	if status == "Playing" then playbackState = "Playing"
	elseif status == "Paused" then playbackState = "Paused"
	else playbackState = "Stopped" end

	playerState = {
		identity = busName,
		name = busName:gsub("org.mpris.MediaPlayer2.", ""),
		playbackState = playbackState,
		loopStatus = playerProps["LoopStatus"] or "None",
		canGoNext = parseBool(playerProps["CanGoNext"]),
		canGoPrevious = parseBool(playerProps["CanGoPrevious"]),
		canControl = parseBool(playerProps["CanControl"]),
		canSeek = parseBool(playerProps["CanSeek"]),
		volume = parseFloat(playerProps["Volume"]),
		position = parseInt(playerProps["Position"]), -- microseconds
		length = parseInt(metadata["mpris:length"]), -- microseconds
		trackTitle = title,
		trackArtists = artists,
		trackAlbum = album,
		artUrl = artUrl,
		trackId = trackId,
		shuffle = parseBool(playerProps["Shuffle"]),
		metadata = metadata,
	}

	publishPlayer()
	publishBackend(true, playerState.name)
end

-- ── Command execution via gdbus ───────────────────────────────────────────────
local function handleCommand(cmd)
	if type(cmd) ~= "table" then return end
	local op = cmd.op
	local player = playerState.identity

	if not player then return end

	local function runGdbusMethod(iface, method, args, cb)
		local argStr = args and (" " .. args) or ""
		local fullCmd = "gdbus call --session --dest " .. shellQuote(player) ..
			" --object-path /org/mpris/MediaPlayer2 " ..
			" --method " .. shellQuote(iface .. "." .. method) .. argStr

		noctalia.runAsync(fullCmd, function(res)
			if res and res.error then
				noctalia.log("[spotify-mpris] gdbus " .. method .. " failed: " .. res.error)
			end
			-- Refresh after any mutating command
			noctalia.runAsync("sleep 0.1", function() discoverPlayer() end)
			if cb then cb(res) end
		end)
	end

	local function runGdbusSet(prop, value, cb)
		local fullCmd = "gdbus call --session --dest " .. shellQuote(player) ..
			" --object-path /org/mpris/MediaPlayer2 " ..
			" --method org.freedesktop.DBus.Properties.Set " ..
			shellQuote("org.mpris.MediaPlayer2.Player") .. " " ..
			shellQuote(prop) .. " " .. shellQuote(value)

		noctalia.runAsync(fullCmd, function(res)
			if res and res.error then
				noctalia.log("[spotify-mpris] gdbus Set " .. prop .. " failed: " .. res.error)
			end
			noctalia.runAsync("sleep 0.1", function() discoverPlayer() end)
			if cb then cb(res) end
		end)
	end

	if op == "play_pause" then
		runGdbusMethod("org.mpris.MediaPlayer2.Player", "PlayPause")
	elseif op == "next" then
		runGdbusMethod("org.mpris.MediaPlayer2.Player", "Next")
	elseif op == "previous" then
		runGdbusMethod("org.mpris.MediaPlayer2.Player", "Previous")
	elseif op == "seek" then
		local offset = tonumber(cmd.offset) or 0 -- microseconds
		runGdbusSet("Position", "<int64 " .. tostring(offset) .. ">")
	elseif op == "volume" then
		local vol = tonumber(cmd.volume) or 0 -- 0.0-1.0
		runGdbusSet("Volume", "<double " .. tostring(vol) .. ">")
	elseif op == "shuffle" then
		local val = cmd.value
		if val == nil then val = not playerState.shuffle end
		runGdbusSet("Shuffle", val and "true" or "false")
	elseif op == "loop" then
		local val = cmd.value
		if val == nil then
			local current = playerState.loopStatus or "None"
			if current == "None" then val = "Track"
			elseif current == "Track" then val = "Playlist"
			else val = "None" end
		end
		runGdbusSet("LoopStatus", shellQuote(val))
	elseif op == "refresh" then
		discoverPlayer()
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
	discoverPlayer()
end

function onConfigChanged()
	noctalia.setUpdateInterval(intervalMs())
	discoverPlayer()
end

function onIpc(event, payload)
	if event == "refresh" then
		discoverPlayer()
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
discoverPlayer()

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