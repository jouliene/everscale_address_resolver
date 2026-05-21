# everscale_address_resolver

ADNL/DHT validator map collector for `validators_clock`.

The resolver reads the current validator set from `validators_clock`, resolves
validator ADNL addresses through native Rust ADNL/DHT, enriches IPs through
`ip-api.com`, and writes map JSON in the format consumed by `validators_clock`.
It was built for Everscale first, but the same resolver can be tested against
TON with the official TON global config.

The public `main.ton.dev` global config is too old for reliable bootstrap. Use a
fresh bootstrap config generated from live Everscale nodes.

## Build

```bash
cargo +stable build --release
```

## Run Once

```bash
cp everscale_address_resolver.example.json everscale_address_resolver.json
target/release/everscale_address_resolver run --config everscale_address_resolver.json --once
```

## TON Experiment

The TON config uses a different chain id and a separate ADNL UDP port so it can
be tested without stopping the Everscale resolver:

```bash
curl -L https://ton-blockchain.github.io/global.config.json -o ton-global.config.json
cp ton_address_resolver.example.json ton_address_resolver.json
target/release/everscale_address_resolver run --config ton_address_resolver.json --once
```

The important fields are:

```json
{
  "chain": "ton",
  "resolver": {
    "global_config_path": "ton-global.config.json",
    "local_adnl_addr": "0.0.0.0:4194"
  }
}
```

## Bootstrap Config

Generate public `dht.node` entries on the server that runs live Everscale nodes.
The private DHT keys stay local; only public signed DHT nodes are written to the
bootstrap config.

The collector expects this file by default:

```text
/home/admin/everscale_dht_bootstrap/everscale-bootstrap-global-config.json
```

## Production Output

Recommended production paths:

```text
/home/admin/.validators_clock/everscale_map/everscale_nodes.json
/home/admin/.validators_clock/everscale_map/everscale_full.json
```

`everscale_full.json` includes resolver metadata with the loaded bootstrap node
count. A missing ADNL IP is reported only after the resolver has swept all
bootstrap DHT peers from the configured global config and still failed to find
an address. `map_stale_after_secs` keeps a previously resolved map node for the
configured grace period, so the default production setting waits 3600 seconds
before the validator disappears from the map output.

## Install And Update

Install after clone:

```bash
./install.sh
```

Update an existing checkout and restart the user service:

```bash
./update.sh
```
