# everscale_address_resolver

Everscale validator map collector for `validators_clock`.

The resolver reads the current Everscale validator set from
`https://validatorsclock.xyz/api/chains/everscale/clock`, resolves validator
ADNL addresses through native Everscale DHT, enriches IPs through `ip-api.com`,
and writes map JSON in the format consumed by `validators_clock`.

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
