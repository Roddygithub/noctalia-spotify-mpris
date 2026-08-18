#!/usr/bin/env bash
set -euo pipefail

# Build script for noctalia-spotify-backend
# Usage: ./build-backend.sh [--install] [--release]

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BACKEND_DIR="$(dirname "$SCRIPT_DIR")"
BUILD_TYPE="release"
INSTALL=false

for arg in "$@"; do
    case $arg in
        --install)
            INSTALL=true
            ;;
        --debug)
            BUILD_TYPE="debug"
            ;;
        *)
            echo "Unknown argument: $arg"
            exit 1
            ;;
    esac
done

echo "Building noctalia-spotify-backend ($BUILD_TYPE)..."

cd "$BACKEND_DIR"

# Build
if [[ "$BUILD_TYPE" == "release" ]]; then
    cargo build --release
else
    cargo build
fi

BINARY="target/$BUILD_TYPE/noctalia-spotify-backend"

if [[ ! -f "$BINARY" ]]; then
    echo "Build failed: binary not found at $BINARY"
    exit 1
fi

echo "Build successful: $BINARY"

if [[ "$INSTALL" == true ]]; then
    echo "Installing..."

    # Install binary
    INSTALL_DIR="$HOME/.local/bin"
    mkdir -p "$INSTALL_DIR"
    cp "$BINARY" "$INSTALL_DIR/noctalia-spotify-backend"
    echo "Installed binary to $INSTALL_DIR/noctalia-spotify-backend"

    # Install systemd service
    SERVICE_DIR="$HOME/.config/systemd/user"
    mkdir -p "$SERVICE_DIR"
    cp systemd/noctalia-spotify-backend.service "$SERVICE_DIR/"
    echo "Installed service to $SERVICE_DIR/noctalia-spotify-backend.service"

    # Reload systemd
    systemctl --user daemon-reload
    echo "Systemd daemon reloaded"

    echo ""
    echo "To enable and start:"
    echo "  systemctl --user enable --now noctalia-spotify-backend"
    echo ""
    echo "To check status:"
    echo "  systemctl --user status noctalia-spotify-backend"
    echo ""
    echo "To view logs:"
    echo "  journalctl --user -u noctalia-spotify-backend -f"
fi

echo "Done!"