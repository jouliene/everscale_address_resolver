#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
CONFIG_PATH="${EVERSCALE_ADDRESS_RESOLVER_CONFIG:-$ROOT/everscale_address_resolver.json}"
SERVICE_NAME="${EVERSCALE_ADDRESS_RESOLVER_SERVICE_NAME:-everscale-address-resolver.service}"
MAP_DIR="${VALIDATORS_CLOCK_EVERSCALE_MAP_DIR:-/home/admin/.validators_clock/everscale_map}"
BOOTSTRAP_CONFIG="${EVERSCALE_BOOTSTRAP_CONFIG:-/home/admin/everscale_dht_bootstrap/everscale-bootstrap-global-config.json}"
BIN="$ROOT/target/release/everscale_address_resolver"

install_apt_packages() {
  if command -v apt-get >/dev/null && command -v sudo >/dev/null; then
    sudo apt-get update
    sudo apt-get install -y "$@"
  fi
}

ensure_rust() {
  if ! command -v rustup >/dev/null; then
    echo "installing Rust toolchain"
    install_apt_packages ca-certificates curl build-essential pkg-config libssl-dev clang
    curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs \
      | sh -s -- -y --default-toolchain stable
    export PATH="$HOME/.cargo/bin:$PATH"
  else
    export PATH="$HOME/.cargo/bin:$PATH"
  fi

  if command -v rustup >/dev/null; then
    echo "updating Rust toolchain"
    rustup update stable
    rustup default stable
  fi
}

ensure_rust

echo "building Everscale address resolver"
cargo +stable build --release

mkdir -p "$MAP_DIR" "$ROOT/out/runtime"

if [[ -f "$CONFIG_PATH" ]]; then
  echo "keeping existing config: $CONFIG_PATH"
else
  echo "creating config: $CONFIG_PATH"
  cat > "$CONFIG_PATH" <<JSON
{
  "base_url": "https://validatorsclock.xyz",
  "interval_secs": 60,
  "full_geo_refresh_secs": 3600,
  "state": "$ROOT/out/runtime/everscale_nodes_state.json",
  "output": "$MAP_DIR/everscale_full.json",
  "map_output": "$MAP_DIR/everscale_nodes.json",
  "map_cache": "$ROOT/out/runtime/everscale_map_cache.json",
  "map_stale_after_secs": 3600,
  "compact": true,
  "resolver": {
    "global_config_path": "$BOOTSTRAP_CONFIG",
    "lookup_timeout_secs": 30
  },
  "geo": {
    "endpoint": "http://ip-api.com/batch?fields=status,message,country,countryCode,regionName,city,lat,lon,isp,org,as,query",
    "batch_size": 100,
    "cache": "$ROOT/out/runtime/everscale_geo_cache.json"
  }
}
JSON
fi

if [[ ! -f "$BOOTSTRAP_CONFIG" ]]; then
  echo "warning: bootstrap config does not exist: $BOOTSTRAP_CONFIG" >&2
  echo "create it with: sudo $ROOT/tools/refresh-bootstrap.sh" >&2
fi

if command -v systemctl >/dev/null; then
  USER_UNIT_DIR="$HOME/.config/systemd/user"
  mkdir -p "$USER_UNIT_DIR"
  UNIT_PATH="$USER_UNIT_DIR/$SERVICE_NAME"
  echo "writing systemd user unit: $UNIT_PATH"
  cat > "$UNIT_PATH" <<UNIT
[Unit]
Description=Everscale validator address resolver
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
WorkingDirectory=$ROOT
ExecStart=$BIN run --config $CONFIG_PATH
Restart=always
RestartSec=10

[Install]
WantedBy=default.target
UNIT
  systemctl --user daemon-reload || true
  echo "service is installed but not started"
  echo "start it with: systemctl --user enable --now $SERVICE_NAME"
fi

echo "installed"
echo "config: $CONFIG_PATH"
echo "run once: $BIN run --config $CONFIG_PATH --once"
