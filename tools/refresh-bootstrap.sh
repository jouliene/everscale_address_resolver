#!/usr/bin/env bash
set -euo pipefail

OUT_DIR="${EVERSCALE_BOOTSTRAP_DIR:-/home/admin/everscale_dht_bootstrap}"
OUT="$OUT_DIR/everscale-bootstrap-global-config.json"
GENDHT="${GENDHT:-/usr/local/bin/gendht}"

if [[ ! -x "$GENDHT" ]]; then
  echo "missing executable gendht at $GENDHT" >&2
  exit 1
fi

mkdir -p "$OUT_DIR/nodes"
chmod 700 "$OUT_DIR"
rm -f "$OUT_DIR"/nodes/*.json

configs=()
for cfg in /home/ever*/.nodekeeper/node/config.json /mnt/data/stever*/.nodekeeper/node/config.json; do
  [[ -f "$cfg" ]] && configs+=("$cfg")
done

if [[ "${#configs[@]}" -eq 0 ]]; then
  echo "no Everscale node configs found" >&2
  exit 1
fi

for cfg in "${configs[@]}"; do
  name="$(printf "%s\n" "$cfg" | sed -E "s#^/(home|mnt/data)/([^/]+)/.*#\2#")"
  ip="$(jq -r ".adnl_node.ip_address" "$cfg")"
  key="$(jq -r ".adnl_node.keys[] | select(.tag == 1) | .data.pvt_key" "$cfg")"

  if [[ -z "$ip" || "$ip" == "null" || -z "$key" || "$key" == "null" ]]; then
    echo "skip $cfg: missing ip or DHT key" >&2
    continue
  fi

  "$GENDHT" "$ip" "$key" > "$OUT_DIR/nodes/$name.json"
done

first_cfg=""
for cfg in "${configs[@]}"; do
  candidate="$(dirname "$cfg")/global-config.json"
  if [[ -f "$candidate" ]]; then
    first_cfg="$candidate"
    break
  fi
done

if [[ -z "$first_cfg" ]]; then
  echo "no global-config.json found near node configs" >&2
  exit 1
fi

jq -n \
  --slurpfile base "$first_cfg" \
  --slurpfile nodes <(jq -s "." "$OUT_DIR"/nodes/*.json) '
  {
    "@type": "config.global",
    "dht": {
      "@type": "dht.config.global",
      "k": ($base[0].dht.k // 6),
      "a": ($base[0].dht.a // 3),
      "static_nodes": {
        "@type": "dht.nodes",
        "nodes": $nodes[0]
      }
    },
    "validator": $base[0].validator
  }' > "$OUT.tmp"

mv "$OUT.tmp" "$OUT"

if [[ -n "${SUDO_USER:-}" ]]; then
  chown -R "$SUDO_USER:$SUDO_USER" "$OUT_DIR"
fi

jq '{
  dht_nodes: (.dht.static_nodes.nodes | length),
  ports: [.dht.static_nodes.nodes[].addr_list.addrs[0].port],
  validator_zero_state: (.validator.zero_state.root_hash != null)
}' "$OUT"
