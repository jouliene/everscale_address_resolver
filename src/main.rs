use std::{
    collections::{BTreeMap, BTreeSet},
    convert::TryInto,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use adnl::{
    DhtNode, DhtSearchPolicy,
    node::{AdnlNode, AdnlNodeConfig},
};
use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand};
use ever_block::{Ed25519KeyOption, KeyId, KeyOption, UInt256, base64_decode};
use serde::{Deserialize, Serialize};
use tokio::time::timeout;
use ton_api::{
    IntoBoxed,
    ton::{
        adnl::{address::address::Udp, addresslist::AddressList as AdnlAddressList},
        dht::node::Node as DhtNodeConfig,
        pub_::publickey::Ed25519,
    },
};

const CHAIN_ID: &str = "everscale";
const LOCAL_ADNL_ADDR: &str = "0.0.0.0:4191";
const DHT_KEY_TAG: usize = 1;

#[derive(Parser)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Run(RunArgs),
}

#[derive(Parser)]
struct RunArgs {
    #[arg(short, long, default_value = "everscale_address_resolver.json")]
    config: PathBuf,
    #[arg(long)]
    once: bool,
}

#[derive(Clone, Debug, Deserialize)]
struct AppConfig {
    #[serde(default = "default_base_url")]
    base_url: String,
    #[serde(default = "default_interval_secs")]
    interval_secs: u64,
    #[serde(default = "default_full_geo_refresh_secs")]
    full_geo_refresh_secs: u64,
    state: PathBuf,
    output: PathBuf,
    map_output: PathBuf,
    map_cache: PathBuf,
    #[serde(default = "default_map_stale_after_secs")]
    map_stale_after_secs: u64,
    #[serde(default)]
    compact: bool,
    resolver: ResolverConfig,
    geo: GeoConfig,
}

#[derive(Clone, Debug, Deserialize)]
struct ResolverConfig {
    global_config_path: PathBuf,
    #[serde(default = "default_lookup_timeout_secs")]
    lookup_timeout_secs: u64,
}

#[derive(Clone, Debug, Deserialize)]
struct GeoConfig {
    #[serde(default = "default_geo_endpoint")]
    endpoint: String,
    #[serde(default = "default_geo_batch_size")]
    batch_size: usize,
    cache: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
struct RuntimeState {
    #[serde(default)]
    last_time_full_check: u64,
    #[serde(default)]
    last_success_at: u64,
    #[serde(default)]
    last_generated_at: u64,
    #[serde(default)]
    last_validators_total: usize,
    #[serde(default)]
    last_validators_with_adnl: usize,
    #[serde(default)]
    last_resolved_total: usize,
    #[serde(default)]
    last_map_nodes: usize,
    #[serde(default)]
    last_geo_lookups: usize,
}

#[derive(Clone, Debug, Deserialize)]
struct ClockResponse {
    chain: ChainInfo,
    fetched_at: u64,
    current_set: ValidatorSet,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ChainInfo {
    id: String,
    name: String,
    color: String,
    token_symbol: String,
    rpc_label: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct ValidatorSet {
    round_id: u64,
    round_color: Option<String>,
    total: usize,
    main: usize,
    validators: Vec<ValidatorApi>,
}

#[derive(Clone, Debug, Deserialize)]
struct ValidatorApi {
    public_key: String,
    adnl_addr: Option<String>,
    wallet: Option<String>,
    source: Option<ValidatorSource>,
    contract_type: Option<String>,
    stake: Option<String>,
    weight: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct ValidatorSource {
    address: String,
    contract_type_hash: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ResolvedAddress {
    ip: String,
    port: i32,
    version: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Resolution {
    status: String,
    addresses: Vec<ResolvedAddress>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl Resolution {
    fn resolved(address: ResolvedAddress) -> Self {
        Self {
            status: "resolved".to_owned(),
            addresses: vec![address],
            error: None,
        }
    }

    fn failed(error: impl Into<String>) -> Self {
        Self {
            status: "failed".to_owned(),
            addresses: Vec::new(),
            error: Some(error.into()),
        }
    }

    fn missing_adnl() -> Self {
        Self {
            status: "missing_adnl".to_owned(),
            addresses: Vec::new(),
            error: Some("validator has no adnl_addr in active set".to_owned()),
        }
    }

    fn invalid_adnl(adnl_addr: &str) -> Self {
        Self {
            status: "invalid_adnl".to_owned(),
            addresses: Vec::new(),
            error: Some(format!("adnl_addr must be 32 bytes hex, got {adnl_addr}")),
        }
    }

    fn is_resolved(&self) -> bool {
        self.status == "resolved" && !self.addresses.is_empty()
    }
}

#[derive(Clone, Debug, Serialize)]
struct FullValidator {
    validator_public_key: String,
    adnl_addr: Option<String>,
    wallet: Option<String>,
    source_address: Option<String>,
    source_contract_type_hash: Option<String>,
    contract_type: Option<String>,
    stake: Option<String>,
    weight: Option<String>,
    resolution: Resolution,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct MapNode {
    peer: String,
    ip: String,
    city: String,
    country: String,
    isp: String,
    lat: f64,
    lon: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct CachedMapNode {
    observed_at: u64,
    node: MapNode,
}

#[derive(Clone, Debug, Serialize)]
struct FullOutput {
    schema_version: u32,
    chain: ChainInfo,
    source_url: String,
    fetched_at: u64,
    generated_at: u64,
    round_id: u64,
    round_color: Option<String>,
    validators_total: usize,
    validators_main: usize,
    validators_with_adnl: usize,
    resolved_total: usize,
    map_nodes: usize,
    resolver: ResolverMetadata,
    validators: Vec<FullValidator>,
}

#[derive(Clone, Debug, Serialize)]
struct ResolverMetadata {
    kind: String,
    attempted_network_resolution: bool,
    global_config_path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct GeoRecord {
    status: String,
    #[serde(default)]
    message: Option<String>,
    query: String,
    #[serde(default)]
    country: Option<String>,
    #[serde(default, rename = "countryCode")]
    country_code: Option<String>,
    #[serde(default, rename = "regionName")]
    region_name: Option<String>,
    #[serde(default)]
    city: Option<String>,
    #[serde(default)]
    lat: Option<f64>,
    #[serde(default)]
    lon: Option<f64>,
    #[serde(default)]
    isp: Option<String>,
    #[serde(default)]
    org: Option<String>,
    #[serde(default, rename = "as")]
    as_name: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct TonNodeGlobalConfigJson {
    dht: DhtGlobalConfig,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct DhtGlobalConfig {
    static_nodes: DhtNodes,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct DhtNodes {
    nodes: Vec<ConfigDhtNode>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ConfigDhtNode {
    id: ConfigDhtNodeId,
    addr_list: ConfigAddressList,
    version: Option<i32>,
    signature: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ConfigDhtNodeId {
    #[serde(alias = "@type")]
    type_node: Option<String>,
    key: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ConfigAddressList {
    addrs: Vec<ConfigAddress>,
    version: Option<i32>,
    reinit_date: Option<i32>,
    priority: Option<i32>,
    expire_at: Option<i32>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ConfigAddress {
    ip: Option<i64>,
    port: Option<u16>,
}

struct EverDhtResolver {
    _adnl: Arc<AdnlNode>,
    dht: Arc<DhtNode>,
    preset_nodes: Vec<Arc<KeyId>>,
    lookup_timeout: Duration,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Run(args) => run(args).await,
    }
}

async fn run(args: RunArgs) -> Result<()> {
    let config = load_config(&args.config)?;
    let base_dir = config_base_dir(&args.config);
    let config = config.resolve_paths(&base_dir);
    let resolver = EverDhtResolver::new(
        &config.resolver.global_config_path,
        Duration::from_secs(config.resolver.lookup_timeout_secs),
    )
    .await?;

    loop {
        if let Err(error) = collect_once(&config, &resolver).await {
            eprintln!("collect error: {error:#}");
            if args.once {
                return Err(error);
            }
        }

        if args.once {
            return Ok(());
        }

        tokio::time::sleep(Duration::from_secs(config.interval_secs)).await;
    }
}

async fn collect_once(config: &AppConfig, resolver: &EverDhtResolver) -> Result<()> {
    let now = unix_now();
    let source_url = format!("{}/api/chains/{CHAIN_ID}/clock", config.base_url);
    eprintln!("collect start chain={CHAIN_ID} resolver=ever-dht");

    let clock = reqwest::get(&source_url)
        .await
        .with_context(|| format!("failed to fetch {source_url}"))?
        .error_for_status()
        .with_context(|| format!("validators clock API returned an error for {source_url}"))?
        .json::<ClockResponse>()
        .await
        .with_context(|| format!("failed to decode validators clock response from {source_url}"))?;

    if clock.chain.id != CHAIN_ID {
        bail!("expected chain id {CHAIN_ID}, got {}", clock.chain.id);
    }

    let mut validators = Vec::with_capacity(clock.current_set.validators.len());
    let mut resolved_total = 0usize;
    let mut validators_with_adnl = 0usize;

    for validator in &clock.current_set.validators {
        let resolution = match validator.adnl_addr.as_deref() {
            Some(adnl_addr) => {
                validators_with_adnl += 1;
                if !is_hex_32(adnl_addr) {
                    Resolution::invalid_adnl(adnl_addr)
                } else {
                    resolver.resolve(adnl_addr).await
                }
            }
            None => Resolution::missing_adnl(),
        };

        if resolution.is_resolved() {
            resolved_total += 1;
        }

        validators.push(FullValidator {
            validator_public_key: validator.public_key.clone(),
            adnl_addr: validator.adnl_addr.clone(),
            wallet: validator.wallet.clone(),
            source_address: validator
                .source
                .as_ref()
                .map(|source| source.address.clone()),
            source_contract_type_hash: validator
                .source
                .as_ref()
                .and_then(|source| source.contract_type_hash.clone()),
            contract_type: validator.contract_type.clone(),
            stake: validator.stake.clone(),
            weight: validator.weight.clone(),
            resolution,
        });
    }

    let mut state = read_json_or_default::<RuntimeState>(&config.state)?;
    let full_geo_refresh =
        now.saturating_sub(state.last_time_full_check) >= config.full_geo_refresh_secs;
    let geo_lookups = refresh_geo_cache(config, &validators, full_geo_refresh).await?;
    let geo_cache = read_json_or_default::<BTreeMap<String, GeoRecord>>(&config.geo.cache)?;
    let map_nodes = build_map_nodes(config, &validators, &geo_cache, now)?;

    let output = FullOutput {
        schema_version: 1,
        chain: clock.chain,
        source_url,
        fetched_at: clock.fetched_at,
        generated_at: now,
        round_id: clock.current_set.round_id,
        round_color: clock.current_set.round_color,
        validators_total: clock.current_set.total,
        validators_main: clock.current_set.main,
        validators_with_adnl,
        resolved_total,
        map_nodes: map_nodes.len(),
        resolver: ResolverMetadata {
            kind: "ever-dht".to_owned(),
            attempted_network_resolution: true,
            global_config_path: config.resolver.global_config_path.display().to_string(),
        },
        validators,
    };

    write_json(&config.output, &output, config.compact)?;
    write_json(&config.map_output, &map_nodes, config.compact)?;

    state.last_success_at = now;
    state.last_generated_at = now;
    state.last_validators_total = output.validators_total;
    state.last_validators_with_adnl = output.validators_with_adnl;
    state.last_resolved_total = output.resolved_total;
    state.last_map_nodes = output.map_nodes;
    state.last_geo_lookups = geo_lookups;
    if full_geo_refresh {
        state.last_time_full_check = now;
    }
    write_json(&config.state, &state, false)?;

    eprintln!(
        "collect ok validators={} with_adnl={} resolved={} map_nodes={} geo_lookups={}",
        output.validators_total,
        output.validators_with_adnl,
        output.resolved_total,
        output.map_nodes,
        geo_lookups
    );

    Ok(())
}

impl AppConfig {
    fn resolve_paths(mut self, base_dir: &Path) -> Self {
        self.state = resolve_config_path(base_dir, &self.state);
        self.output = resolve_config_path(base_dir, &self.output);
        self.map_output = resolve_config_path(base_dir, &self.map_output);
        self.map_cache = resolve_config_path(base_dir, &self.map_cache);
        self.resolver.global_config_path =
            resolve_config_path(base_dir, &self.resolver.global_config_path);
        self.geo.cache = resolve_config_path(base_dir, &self.geo.cache);
        self
    }
}

impl EverDhtResolver {
    async fn new(global_config_path: &Path, lookup_timeout: Duration) -> Result<Self> {
        let config = read_global_config(global_config_path)?;
        let dht_nodes = config.get_dht_nodes_configs()?;

        let (_, adnl_config) = AdnlNodeConfig::with_ip_address_and_private_key_tags(
            LOCAL_ADNL_ADDR,
            vec![DHT_KEY_TAG],
        )
        .context("failed to create local ADNL config")?;
        let adnl = AdnlNode::with_config(adnl_config)
            .await
            .context("failed to create local ADNL node")?;
        let dht = DhtNode::with_params(adnl.clone(), DHT_KEY_TAG, None)
            .context("failed to create DHT node")?;
        AdnlNode::start(&adnl, vec![dht.clone()])
            .await
            .context("failed to start ADNL node")?;

        let mut preset_nodes = Vec::new();
        for dht_node in &dht_nodes {
            if let Some(key) = dht
                .add_peer_to_network(dht_node, None)
                .context("failed to add DHT bootstrap peer")?
            {
                preset_nodes.push(key);
            }
        }

        if preset_nodes.is_empty() {
            bail!("bootstrap config has no valid DHT static nodes");
        }

        Ok(Self {
            _adnl: adnl,
            dht,
            preset_nodes,
            lookup_timeout,
        })
    }

    async fn resolve(&self, adnl_addr: &str) -> Resolution {
        match timeout(self.lookup_timeout, self.resolve_inner(adnl_addr)).await {
            Ok(Ok(address)) => Resolution::resolved(address),
            Ok(Err(error)) => Resolution::failed(error.to_string()),
            Err(_) => Resolution::failed(format!(
                "lookup timed out after {}s",
                self.lookup_timeout.as_secs()
            )),
        }
    }

    async fn resolve_inner(&self, adnl_addr: &str) -> Result<ResolvedAddress> {
        let adnl_key = hex::decode(adnl_addr).context("invalid adnl hex")?;
        let key_id = KeyId::from_data(
            adnl_key
                .as_slice()
                .try_into()
                .map_err(|_| anyhow!("adnl key must be 32 bytes"))?,
        );

        let mut context = None;
        let mut nodes = self.preset_nodes.clone();
        let mut bad_nodes = Vec::<Arc<KeyId>>::new();
        let mut index = 0usize;

        loop {
            if let Ok(Some((ip, _key))) = DhtNode::find_address_in_network_with_context(
                &self.dht,
                &key_id,
                &mut context,
                DhtSearchPolicy::FastSearch(5),
                None,
            )
            .await
            {
                return endpoint_from_display(&ip.to_string());
            }

            if index >= nodes.len() {
                nodes.clear();
                for node in self
                    .dht
                    .get_known_nodes_of_network(10000, None)
                    .context("failed to read known DHT nodes")?
                {
                    if let Some(key) = self
                        .dht
                        .add_peer_to_network(&node, None)
                        .context("failed to add known DHT node")?
                    {
                        if !bad_nodes.contains(&key) {
                            nodes.push(key);
                        }
                    }
                }

                if nodes.is_empty() {
                    bail!("no responsive DHT peers");
                }
                index = 0;
            }

            let node_key = nodes[index].clone();
            match self.dht.find_dht_nodes_in_network(&node_key, None).await {
                Ok(true) => index += 1,
                Ok(false) | Err(_) => {
                    bad_nodes.push(nodes.remove(index));
                }
            }
        }
    }
}

fn endpoint_from_display(value: &str) -> Result<ResolvedAddress> {
    let (ip, port) = value
        .rsplit_once(':')
        .ok_or_else(|| anyhow!("DHT returned address without port: {value}"))?;
    Ok(ResolvedAddress {
        ip: ip.to_owned(),
        port: port
            .parse::<i32>()
            .with_context(|| format!("DHT returned invalid port in {value}"))?,
        version: "udp4".to_owned(),
    })
}

async fn refresh_geo_cache(
    config: &AppConfig,
    validators: &[FullValidator],
    full_refresh: bool,
) -> Result<usize> {
    let mut cache = read_json_or_default::<BTreeMap<String, GeoRecord>>(&config.geo.cache)?;
    let ips: BTreeSet<String> = validators
        .iter()
        .flat_map(|validator| validator.resolution.addresses.iter())
        .map(|address| address.ip.clone())
        .collect();

    let check_ips: Vec<String> = ips
        .into_iter()
        .filter(|ip| full_refresh || !cache.contains_key(ip))
        .collect();

    if check_ips.is_empty() {
        return Ok(0);
    }

    let client = reqwest::Client::new();
    let batch_size = config.geo.batch_size.max(1);
    let mut lookups = 0usize;

    for chunk in check_ips.chunks(batch_size) {
        let response = client
            .post(&config.geo.endpoint)
            .json(&chunk)
            .send()
            .await
            .with_context(|| format!("failed to call geo endpoint {}", config.geo.endpoint))?
            .error_for_status()
            .with_context(|| format!("geo endpoint returned error {}", config.geo.endpoint))?
            .json::<Vec<GeoRecord>>()
            .await
            .context("failed to decode geo response")?;

        lookups += chunk.len();
        for record in response {
            if record.status == "success" {
                cache.insert(record.query.clone(), record);
            }
        }
    }

    write_json(&config.geo.cache, &cache, false)?;
    Ok(lookups)
}

fn build_map_nodes(
    config: &AppConfig,
    validators: &[FullValidator],
    geo_cache: &BTreeMap<String, GeoRecord>,
    now: u64,
) -> Result<Vec<MapNode>> {
    let mut cache = read_json_or_default::<BTreeMap<String, CachedMapNode>>(&config.map_cache)?;
    let mut output = Vec::new();

    for validator in validators {
        let peer = validator.validator_public_key.to_lowercase();
        if let Some(node) = validator
            .resolution
            .addresses
            .iter()
            .find_map(|address| map_node_for_address(&peer, address, geo_cache))
        {
            cache.insert(
                peer,
                CachedMapNode {
                    observed_at: now,
                    node: node.clone(),
                },
            );
            output.push(node);
            continue;
        }

        if let Some(cached) = cache.get(&peer) {
            if now.saturating_sub(cached.observed_at) <= config.map_stale_after_secs {
                output.push(cached.node.clone());
            }
        }
    }

    output.sort_by(|a, b| {
        a.country
            .cmp(&b.country)
            .then(a.city.cmp(&b.city))
            .then(a.ip.cmp(&b.ip))
            .then(a.peer.cmp(&b.peer))
    });
    write_json(&config.map_cache, &cache, false)?;
    Ok(output)
}

fn map_node_for_address(
    peer: &str,
    address: &ResolvedAddress,
    geo_cache: &BTreeMap<String, GeoRecord>,
) -> Option<MapNode> {
    let geo = geo_cache.get(&address.ip)?;
    Some(MapNode {
        peer: peer.to_owned(),
        ip: address.ip.clone(),
        city: geo.city.clone().unwrap_or_default(),
        country: geo.country.clone().unwrap_or_default(),
        isp: geo.isp.clone().unwrap_or_default(),
        lat: geo.lat?,
        lon: geo.lon?,
    })
}

impl TonNodeGlobalConfigJson {
    fn get_dht_nodes_configs(&self) -> Result<Vec<DhtNodeConfig>> {
        let mut result = Vec::new();
        for dht_node in &self.dht.static_nodes.nodes {
            let key = dht_node.id.convert_key()?;
            let mut addrs = Vec::new();
            for addr in &dht_node.addr_list.addrs {
                let Some(ip) = addr.ip else {
                    continue;
                };
                let Some(port) = addr.port else {
                    continue;
                };
                addrs.push(
                    Udp {
                        ip: ip as i32,
                        port: port as i32,
                    }
                    .into_boxed(),
                );
            }

            let Some(version) = dht_node.addr_list.version else {
                continue;
            };
            let Some(reinit_date) = dht_node.addr_list.reinit_date else {
                continue;
            };
            let Some(priority) = dht_node.addr_list.priority else {
                continue;
            };
            let Some(expire_at) = dht_node.addr_list.expire_at else {
                continue;
            };
            let Some(node_version) = dht_node.version else {
                continue;
            };
            let Some(signature) = &dht_node.signature else {
                continue;
            };

            result.push(DhtNodeConfig {
                id: Ed25519 {
                    key: UInt256::with_array(key.pub_key()?.try_into()?),
                }
                .into_boxed(),
                addr_list: AdnlAddressList {
                    addrs,
                    version,
                    reinit_date,
                    priority,
                    expire_at,
                },
                version: node_version,
                signature: base64_decode(signature)?,
            });
        }
        Ok(result)
    }
}

impl ConfigDhtNodeId {
    fn convert_key(&self) -> Result<Arc<dyn KeyOption>> {
        let type_node = self
            .type_node
            .as_deref()
            .ok_or_else(|| anyhow!("DHT node key type is missing"))?;
        if type_node != "pub.ed25519" {
            bail!("unsupported DHT node key type {type_node}");
        }

        let key = self
            .key
            .as_deref()
            .ok_or_else(|| anyhow!("DHT node public key is missing"))
            .and_then(|key| base64_decode(key).map_err(Into::into))?;
        let pub_key = key
            .get(..32)
            .ok_or_else(|| anyhow!("DHT node public key is shorter than 32 bytes"))?
            .try_into()?;
        Ok(Ed25519KeyOption::from_public_key(pub_key))
    }
}

fn read_global_config(path: &Path) -> Result<TonNodeGlobalConfigJson> {
    let file = fs::File::open(path)
        .with_context(|| format!("failed to open global config {}", path.display()))?;
    serde_json::from_reader(file)
        .with_context(|| format!("failed to parse global config {}", path.display()))
}

fn load_config(path: &Path) -> Result<AppConfig> {
    let file = fs::File::open(path)
        .with_context(|| format!("failed to read config {}", path.display()))?;
    serde_json::from_reader(file)
        .with_context(|| format!("failed to parse config {}", path.display()))
}

fn read_json_or_default<T>(path: &Path) -> Result<T>
where
    T: for<'de> Deserialize<'de> + Default,
{
    if !path.exists() {
        return Ok(T::default());
    }
    let file =
        fs::File::open(path).with_context(|| format!("failed to read json {}", path.display()))?;
    serde_json::from_reader(file)
        .with_context(|| format!("failed to parse json {}", path.display()))
}

fn write_json<T: Serialize>(path: &Path, value: &T, compact: bool) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory {}", parent.display()))?;
    }

    let tmp = temp_path(path);
    let data = if compact {
        serde_json::to_vec(value)?
    } else {
        serde_json::to_vec_pretty(value)?
    };
    fs::write(&tmp, data).with_context(|| format!("failed to write {}", tmp.display()))?;
    fs::rename(&tmp, path)
        .with_context(|| format!("failed to move {} to {}", tmp.display(), path.display()))?;
    Ok(())
}

fn temp_path(path: &Path) -> PathBuf {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    PathBuf::from(tmp)
}

fn config_base_dir(path: &Path) -> PathBuf {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf()
}

fn resolve_config_path(base_dir: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base_dir.join(path)
    }
}

fn is_hex_32(value: &str) -> bool {
    value.len() == 64 && value.as_bytes().iter().all(|byte| byte.is_ascii_hexdigit())
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn default_base_url() -> String {
    "https://validatorsclock.xyz".to_owned()
}

fn default_interval_secs() -> u64 {
    60
}

fn default_full_geo_refresh_secs() -> u64 {
    3600
}

fn default_lookup_timeout_secs() -> u64 {
    30
}

fn default_map_stale_after_secs() -> u64 {
    3600
}

fn default_geo_endpoint() -> String {
    "http://ip-api.com/batch?fields=status,message,country,countryCode,regionName,city,lat,lon,isp,org,as,query".to_owned()
}

fn default_geo_batch_size() -> usize {
    100
}
