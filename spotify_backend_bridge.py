#!/usr/bin/env python3
"""
Bridge Noctalia <-> omarchy-spotify-backend Unix socket.
Reads JSON requests from stdin, sends to backend socket, prints JSON response to stdout.
Used via noctalia.runAsync("python3 spotify_backend_bridge.py", callback)
"""
import json
import socket
import sys
import os

SOCKET_PATH = os.environ.get(
    "SPOTIFY_BACKEND_SOCKET",
    os.path.join(os.environ.get("XDG_RUNTIME_DIR", f"/run/user/{os.getuid()}"), "noctalia-spotify/backend.sock")
)

def send_request(req: dict) -> dict:
    try:
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as s:
            s.settimeout(3)
            s.connect(SOCKET_PATH)
            s.sendall((json.dumps(req) + "\n").encode())
            # Read response (single line JSON)
            data = b""
            while True:
                chunk = s.recv(4096)
                if not chunk:
                    break
                data += chunk
                if b"\n" in data:
                    break
            return json.loads(data.decode().strip())
    except FileNotFoundError:
        return {"v": 1, "id": req.get("id", 0), "ok": False, "error": {"code": "socket_not_found", "message": f"Backend socket not found at {SOCKET_PATH}. Is noctalia-spotify-backend running?"}}
    except ConnectionRefusedError:
        return {"v": 1, "id": req.get("id", 0), "ok": False, "error": {"code": "connection_refused", "message": "Backend socket connection refused. Is noctalia-spotify-backend running?"}}
    except socket.timeout:
        return {"v": 1, "id": req.get("id", 0), "ok": False, "error": {"code": "timeout", "message": "Backend request timed out"}}
    except Exception as e:
        return {"v": 1, "id": req.get("id", 0), "ok": False, "error": {"code": "bridge_error", "message": str(e)}}

def main():
    if len(sys.argv) > 1:
        # Single request mode: python3 bridge.py '{"v":1,"id":1,"command":"get_state"}'
        req = json.loads(sys.argv[1])
        print(json.dumps(send_request(req)))
    else:
        # Stdin/stdout line-delimited mode for persistent bridge
        for line in sys.stdin:
            line = line.strip()
            if not line:
                continue
            try:
                req = json.loads(line)
                resp = send_request(req)
                print(json.dumps(resp), flush=True)
            except Exception as e:
                print(json.dumps({"v": 1, "id": 0, "ok": False, "error": {"code": "parse_error", "message": str(e)}}), flush=True)

if __name__ == "__main__":
    main()