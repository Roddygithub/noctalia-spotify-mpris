-- service.luau
-- Spotify MPRIS - background service that polls MPRIS via gdbus (no playerctl dependency).
--
-- Architecture: Noctalia plugin scripts run in an isolated sandbox with NO
-- `require`/`dofile`/`loadfile`. All MPRIS interaction lives here and
-- is published to UI entries (widget/panel) as plain data via `noctalia.state`.

local PLUGIN_ID = "roddygithub/spotify-mpris"

-- ── State contract (read by UI entries) ───────────────────────────────────────
--   noctalia.state "sp.backend"   : { available=bool, name=string, type=string }
--   noctalia.state "sp.player"    : { identity, name, playbackState, loopStatus, canGoNext, canGoPrevious,
--                                     canControl, canSeek, volume, position, length,
--                                     trackTitle, trackArtists, trackAlbum, artUrl, artPath,
--                                     trackId, shuffle, metadata }
--   noctalia.state "sp.cmd"       : { op=string, [args...] }
--                                   written by UI entries; service executes and clears.
--                                   ops: play_pause, next, previous, seek, volume, shuffle, loop

-- ── Translations ──────────────────────────────────────────────────────────────
local TRANSLATIONS = {
	en = {
		["settings.update_interval.label"] = "Update Interval",
		["settings.update_interval.description"] = "Polling interval for MPRIS updates (milliseconds)",
		["settings.show_artist.label"] = "Show Artist",
		["settings.show_artist.description"] = "Show artist name in bar widget",
		["settings.show_album_art.label"] = "Show Album Art",
		["settings.show_album_art.description"] = "Show album art in panel",
		["settings.filter_player.label"] = "Player Filter",
		["settings.filter_player.description"] = "Which MPRIS player to use",
		["settings.filter_player.any"] = "Any MPRIS Player",
		["settings.filter_player.spotify"] = "Spotify Only",
		["settings.backend_mode.label"] = "Backend Mode",
		["settings.backend_mode.description"] = "Which backend to use for Spotify control",
		["settings.backend_mode.auto"] = "Auto (prefer native, fallback to MPRIS)",
		["settings.backend_mode.native"] = "Native (librespot via Unix socket)",
		["settings.backend_mode.mpris"] = "MPRIS (system D-Bus via gdbus)",
		["settings.language.label"] = "Language",
		["settings.language.description"] = "Interface language",
		["settings.language.en"] = "English",
		["settings.language.fr"] = "Français",
	},
	fr = {
		["settings.update_interval.label"] = "Intervalle de mise à jour",
		["settings.update_interval.description"] = "Intervalle de polling pour les mises à jour MPRIS (millisecondes)",
		["settings.show_artist.label"] = "Afficher l'artiste",
		["settings.show_artist.description"] = "Afficher le nom de l'artiste dans le widget de la barre",
		["settings.show_album_art.label"] = "Afficher la pochette",
		["settings.show_album_art.description"] = "Afficher la pochette d'album dans le panneau",
		["settings.filter_player.label"] = "Filtre du lecteur",
		["settings.filter_player.description"] = "Quel lecteur MPRIS utiliser",
		["settings.filter_player.any"] = "Tout lecteur MPRIS",
		["settings.filter_player.spotify"] = "Spotify uniquement",
		["settings.backend_mode.label"] = "Mode backend",
		["settings.backend_mode.description"] = "Quel backend utiliser pour contrôler Spotify",
		["settings.backend_mode.auto"] = "Auto (préférer natif, repli MPRIS)",
		["settings.backend_mode.native"] = "Natif (librespot via socket Unix)",
		["settings.backend_mode.mpris"] = "MPRIS (D-Bus système via gdbus)",
		["settings.language.label"] = "Langue",
		["settings.language.description"] = "Langue de l'interface",
		["settings.language.en"] = "Anglais",
		["settings.language.fr"] = "Français",
	},
}

local function getTranslationTable()
	local lang = noctalia.getConfig("language") or "en"
	return TRANSLATIONS[lang] or TRANSLATIONS.en
end

-- Initialize translation state
noctalia.state.set("sp.trTable", getTranslationTable())

-- ── Helpers ────────────────────────────────────────────────────────────────────
local function shellQuote(s)
	return "'" .. tostring(s):gsub("'", "'\\''") .. "'"
end

local function parseGdbusScalar(s)
	s = s:gsub("^%s+", ""):gsub("%s+$", "")
	if s == "true" then return true end
	if s == "false" then return false end
	-- typed ints like "int64 59148000", "uint32 3"
	local typed = s:match("^[%a_]+%s+(%-?%d+)$")
	if typed then return tonumber(typed) end
	local n = tonumber(s)
	if n then return n end
	-- quoted string: '...' or "..."
	local q = s:match("^'(.*)'$") or s:match('^"(.*)"$')
	if q then return q end
	return s
end

local function parseGdbusArray(raw)
	if not raw then return {} end
	local arr = raw:match("^%[(.*)%]$")
	if not arr then return {} end
	local out = {}
	local pos = 1
	while pos <= #arr do
		local c = arr:sub(pos, pos)
		if c == "'" or c == '"' then
			local endq = arr:find(c, pos + 1, true)
			if endq then
				table.insert(out, arr:sub(pos + 1, endq - 1))
				pos = endq + 1
			else
				pos = pos + 1
			end
		else
			pos = pos + 1
		end
	end
	return out
end

local function parseGdbusValue(raw)
	if not raw then return nil end
	raw = raw:gsub("^%s+", ""):gsub("%s+$", "")
	-- strip outer parens: (...,)
	local content = raw:match("^%((.*)%)$")
	if content then
		content = content:gsub(",%s*$", "")
	else
		content = raw
	end
	-- strip angle-bracket variant wrapper: <...>
	content = content:match("^<(.+)>$") or content
	if content:sub(1, 1) == "[" then
		return parseGdbusArray(content)
	end
	return parseGdbusScalar(content)
end

local function parseGdbusDict(raw)
	-- Parse a{sv} dict like: ({'key': <'value'>, ...},)
	if not raw then return {} end
	local out = {}
	for k, v in raw:gmatch("'([^']+)'%s*:%s*<([^>]+)>") do
		out[k] = parseGdbusValue(v)
	end
	return out
end

-- ── Python bridge ───────────────────────────────────────────────────────────────
local function getPythonBridgePath()
	-- Use plugin data directory + script name (installed alongside service.luau)
	local pluginDir = noctalia.pluginDataDir()
	if pluginDir and pluginDir ~= "" then
		return pluginDir .. "/spotify_backend_bridge.py"
	end
	-- Fallback: try relative to current working directory
	return "spotify_backend_bridge.py"
end

local function getBackendSocketPath()
	local runtimeDir = os.getenv("XDG_RUNTIME_DIR") or ("/run/user/" .. tostring(os.getuid()))
	return runtimeDir .. "/noctalia-spotify/backend.sock"
end

local function runNativeBridge(request, onDone)
	local scriptPath = getPythonBridgePath()
	-- Override socket path via environment
	local jsonStr = noctalia.json.encode(request)
	local cmd = "SPOTIFY_BACKEND_SOCKET=" .. shellQuote(getBackendSocketPath()) .. " python3 " .. shellQuote(scriptPath) .. " " .. shellQuote(jsonStr)
	noctalia.runAsync(cmd, function(res)
		if res and res.stdout then
			local ok, data = pcall(noctalia.json.decode, res.stdout)
			if ok then
				onDone(data)
				return
			end
		end
		noctalia.log("[spotify-mpris] native bridge error: " .. tostring(res and res.error or "no output"))
		onDone({ ok = false, error = res and res.error or "bridge failed" })
	end)
end


local function convertPythonStateToPlayer(pyState)
	if not pyState or not pyState.ok then
		return nil
	end
	local data = pyState.data or {}
	
	-- Map Python backend state to our player state format
	local playing = data.status == "Playing"
	local shuffle = data.shuffle == true
	local loopStatus = data.loop_status or "None"
	
	return {
		identity = "python-backend",
		name = "Spotify (Python backend)",
		playbackState = data.status or "Stopped",
		loopStatus = loopStatus,
		canGoNext = true,
		canGoPrevious = true,
		canControl = true,
		canSeek = true,
		volume = tonumber(data.volume) or 0.5,
		position = tonumber(data.position) or 0,
		length = tonumber(data.duration) or 0,
		trackTitle = data.title or "",
		trackArtists = data.artist and {data.artist} or {},
		trackAlbum = data.album or "",
		artUrl = data.art_url or "",
		artPath = nil, -- Will be downloaded async
		trackId = data.track_id or "",
		shuffle = shuffle,
		metadata = {
			["xesam:title"] = data.title or "",
			["xesam:artist"] = data.artist and {data.artist} or {},
			["xesam:album"] = data.album or "",
			["mpris:artUrl"] = data.art_url or "",
			["mpris:trackid"] = data.track_id or "",
			["mpris:length"] = tostring((tonumber(data.duration) or 0) * 1000000),
		},
	}
end

local function fetchNativeBackendState()
	runNativeBridge({ v = 1, id = 1, command = "get_state" }, function(resp)
		local player = convertPythonStateToPlayer(resp)
		if player then
			playerState = player
			publishPlayer()
			publishBackend(true, player.name, "native")
			
			-- Kick off async art download
			if player.artUrl and player.artUrl:sub(1, 4) == "http" and player.trackId ~= lastArtTrackId then
				lastArtTrackId = player.trackId
				downloadArtwork(player.artUrl, player.trackId, function(path)
					if path then
						artCache[player.trackId] = path
						playerState.artPath = path
						publishPlayer()
					end
				end)
			end
		else
			publishBackend(false, "Native backend unavailable", "native")
		end
	end)
end

local function sendNativeCommand(cmd)
	local op = cmd.op
	local pyCmd = nil
	
	if op == "play_pause" then
		pyCmd = "play_pause"
	elseif op == "next" then
		pyCmd = "next"
	elseif op == "previous" then
		pyCmd = "previous"
	elseif op == "seek" then
		pyCmd = "seek"
	elseif op == "volume" then
		pyCmd = "set_volume"
	elseif op == "shuffle" then
		pyCmd = "shuffle"
	elseif op == "loop" then
		pyCmd = "repeat"
	elseif op == "authenticate" then
		pyCmd = "authenticate"
	elseif op == "refresh" then
		fetchNativeBackendState()
		return
	else
		noctalia.log("[spotify-mpris] unknown native cmd op: " .. tostring(op))
		return
	end
	
	local request = { v = 1, id = 2, command = pyCmd }
	if op == "seek" then
		request.args = { position = tonumber(cmd.offset) or 0 }
	elseif op == "volume" then
		request.args = { volume = tonumber(cmd.volume) or 0 }
	elseif op == "shuffle" then
		request.args = { enabled = cmd.value }
	elseif op == "loop" then
		request.args = { mode = cmd.value }
	elseif op == "authenticate" then
		-- No args needed, just triggers auth flow
	end
	
	runNativeBridge(request, function(resp)
		if resp and resp.ok then
			noctalia.runAsync("sleep 0.1", function() fetchNativeBackendState() end)
		else
			noctalia.log("[spotify-mpris] native cmd failed: " .. tostring(resp and resp.error))
		end
	end)
end
-- Download HTTPS album art to local temp file (async via noctalia.runAsync)
local function downloadArtwork(url, trackId, onDone)
	if not url or url == "" then
		if onDone then onDone(nil) end
		return
	end
	if url:sub(1, 7) == "file://" then
		if onDone then onDone(url:sub(8)) end
		return
	end
	if url:sub(1, 4) ~= "http" then
		if onDone then onDone(nil) end
		return
	end
	local filename = "/tmp/spotify-mpris-art-" .. tostring(trackId or "unknown"):gsub("[^%w]", "_") .. ".jpg"
	
	-- Try curl first, fall back to wget
	local function tryDownload(cmd, toolName)
		noctalia.runAsync(cmd, function(res)
			if res and noctalia.fileExists(filename) then
				noctalia.log("[spotify-mpris] album art downloaded via " .. toolName .. ": " .. filename)
				if onDone then onDone(filename) end
			else
				noctalia.log("[spotify-mpris] album art download failed via " .. toolName .. " for " .. url .. " err=" .. tostring(res and res.error))
				-- If curl fails, try wget
				if toolName == "curl" then
					local wgetCmd = "wget -q --timeout=5 -O " .. filename .. " " .. url
					tryDownload(wgetCmd, "wget")
				else
					if onDone then onDone(nil) end
				end
			end
		end)
	end
	
	local curlCmd = "curl -sL --max-time 5 -o " .. filename .. " " .. url
	tryDownload(curlCmd, "curl")
end

-- ── Player state ──────────────────────────────────────────────────────────────
local playerState = {}
local lastPlayerIdentity = nil
local lastArtTrackId = nil
local artCache = {} -- trackId -> local file path

local function publishPlayer()
	noctalia.state.set("sp.player", playerState)
end

local function publishBackend(available, name, backendType)
	noctalia.state.set("sp.backend", { available = available, name = name or "None", type = backendType or "gdbus" })
end

-- ── Fetch all metadata via gdbus ──────────────────────────────────────────────
-- DEFINED BEFORE discoverPlayer because discoverPlayer calls fetchMetadata
local function buildPlayerState(busName, metadata, playerProps)
	-- Extract track info from metadata
	local title = metadata["xesam:title"] or ""
	local artists = metadata["xesam:artist"] or {}
	local album = metadata["xesam:album"] or ""
	local artUrl = metadata["mpris:artUrl"] or ""
	local trackId = metadata["mpris:trackid"] or ""

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

	-- artPath is resolved synchronously only for file:// URLs; HTTPS art is
	-- downloaded async once per track and cached so later polls keep it.
	local artPath = nil
	if artUrl:sub(1, 7) == "file://" then
		artPath = artUrl:sub(8)
	elseif artCache[trackId] then
		artPath = artCache[trackId]
	end

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
		artPath = artPath,
		trackId = trackId,
		shuffle = parseBool(playerProps["Shuffle"]),
		metadata = metadata,
	}

	publishPlayer()
	publishBackend(true, playerState.name, "gdbus")

	-- Kick off async HTTPS art download once per track
	if artPath == nil and artUrl:sub(1, 4) == "http" and trackId ~= lastArtTrackId then
		lastArtTrackId = trackId
		downloadArtwork(artUrl, trackId, function(path)
			if path then
				artCache[trackId] = path
				playerState.artPath = path
				publishPlayer()
			end
		end)
	end
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
			buildPlayerState(busName, metadata, playerProps)
		end
	end)
end

local function fetchMetadata(busName)
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

-- ── Discover MPRIS player via gdbus ─────────────────────────────────────────────
local function discoverMprisPlayer()
	local filter = noctalia.getConfig("filter_player") or "any"
	local cmd
	if filter == "spotify" then
		cmd = "gdbus call --session --dest org.freedesktop.DBus --object-path /org/freedesktop/DBus --method org.freedesktop.DBus.ListNames | grep -o 'org.mpris.MediaPlayer2.spotify[^,]*'"
	else
		cmd = "gdbus call --session --dest org.freedesktop.DBus --object-path /org/freedesktop/DBus --method org.freedesktop.DBus.ListNames | grep -o 'org.mpris.MediaPlayer2[^,]*' | head -1"
	end

	noctalia.runAsync(cmd, function(res)
		if not res or res.error or not res.stdout or res.stdout == "" then
			if next(playerState) ~= nil then
				playerState = {}
				publishPlayer()
			end
			publishBackend(false, "No MPRIS player", "gdbus")
			return
		end

		-- Extract bus name, handling trailing quote from grep output
		local busName = res.stdout:match("^%s*(.-)%s*'") or res.stdout:match("^%s*(.-)%s*$")

		if busName == lastPlayerIdentity and next(playerState) ~= nil then
			fetchMetadata(busName)
			return
		end

		lastPlayerIdentity = busName
		fetchMetadata(busName)
	end)
end

-- ── Discover player via selected backend ──────────────────────────────────────
local function discoverPlayer()
	local backendMode = noctalia.getConfig("backend_mode") or "auto"
	
	if backendMode == "native" then
		fetchNativeBackendState()
		return
	elseif backendMode == "mpris" then
		discoverMprisPlayer()
		return
	end
	
	-- auto: try native first, then fall back to MPRIS
	runNativeBridge({ v = 1, id = 0, command = "ping" }, function(resp)
		if resp and resp.ok then
			fetchNativeBackendState()
		else
			discoverMprisPlayer()
		end
	end)
end

-- ── Command execution ─────────────────────────────────────────────────────────
local function handleCommand(cmd)
	if type(cmd) ~= "table" then return end
	local op = cmd.op
	local backendMode = noctalia.getConfig("backend_mode") or "auto"
	
	-- Route to appropriate backend based on current mode
	if backendMode == "native" or (backendMode == "auto" and playerState.identity == "python-backend") then
		sendNativeCommand(cmd)
		return
	elseif backendMode == "mpris" or (backendMode == "auto" and playerState.identity ~= "python-backend" and playerState.identity) then
		-- gdbus backend (original logic)
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
			local offset = tonumber(cmd.offset) or 0
			runGdbusSet("Position", "<int64 " .. tostring(offset) .. ">")
		elseif op == "volume" then
			local vol = tonumber(cmd.volume) or 0
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
		return
	end
	
	-- Default: try native first if available
	if op == "authenticate" then
		sendNativeCommand(cmd)
		return
	end
	
	-- Fall through to gdbus for other commands if no native backend
	local player = playerState.identity
	if not player then
		noctalia.log("[spotify-mpris] no player available for command: " .. tostring(op))
		return
	end
	
	-- gdbus backend (original logic)
	local function runGdbusMethod(iface, method, args, cb)
		local argStr = args and (" " .. args) or ""
		local fullCmd = "gdbus call --session --dest " .. shellQuote(player) ..
			" --object-path /org/mpris/MediaPlayer2 " ..
			" --method " .. shellQuote(iface .. "." .. method) .. argStr

		noctalia.runAsync(fullCmd, function(res)
			if res and res.error then
				noctalia.log("[spotify-mpris] gdbus " .. method .. " failed: " .. res.error)
			end
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
		local offset = tonumber(cmd.offset) or 0
		runGdbusSet("Position", "<int64 " .. tostring(offset) .. ">")
	elseif op == "volume" then
		local vol = tonumber(cmd.volume) or 0
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
	
	-- Update translations if language changed
	noctalia.state.set("sp.trTable", getTranslationTable())
	
	-- Clear state when backend changes
	playerState = {}
	lastPlayerIdentity = nil
	lastArtTrackId = nil
	artCache = {}
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
local lastCmdSeq = 0
noctalia.state.watch("sp.cmd", function(cmd)
	if type(cmd) ~= "table" then return end
	local seq = tonumber(cmd.seq) or 0
	if seq <= lastCmdSeq then return end
	lastCmdSeq = seq
	cmd.seq = nil
	handleCommand(cmd)
end)