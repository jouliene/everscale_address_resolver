#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")"

SERVICE_NAME="${EVERSCALE_ADDRESS_RESOLVER_SERVICE_NAME:-everscale-address-resolver.service}"

echo "Updating Git checkout with fast-forward only"
git pull --ff-only

echo "Installing updated resolver"
./install.sh

if command -v systemctl >/dev/null; then
  if systemctl --user is-active --quiet "$SERVICE_NAME"; then
    echo "Restarting $SERVICE_NAME"
    systemctl --user restart "$SERVICE_NAME"
  elif systemctl --user is-enabled --quiet "$SERVICE_NAME"; then
    echo "Starting enabled $SERVICE_NAME"
    systemctl --user start "$SERVICE_NAME"
  else
    echo "Service is installed but not enabled"
    echo "Start it with: systemctl --user enable --now $SERVICE_NAME"
  fi
fi

echo "updated"
