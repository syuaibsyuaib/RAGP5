
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyDict};
use std::collections::{HashMap, HashSet, VecDeque};
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use lru::LruCache;
use sysinfo::System;
use tokio::runtime::{Builder as TokioRuntimeBuilder, Runtime as TokioRuntime};
use tokio::sync::{mpsc, oneshot, Mutex as TokioMutex};

const MAGIC_BASE: u32 = 0x5241_4750; // "RAGP"
const MAGIC_DELTA: u32 = 0x4445_4C54; // "DELT"
const VERSION: u16 = 1;

const BASE_HEADER_SIZE: u64 = 14;
const NODE_INDEX_SIZE: u64 = 32;
const SYNAPSE_SIZE: u64 = 12;
const DELTA_HEADER_SIZE: u64 = 8;
const DELTA_ENTRY_SIZE: u64 = 28;
const CHUNK_SPAN: u64 = 100;
const OFFSET_CHUNK_FLAG: u64 = 1_u64 << 63;

const MAX_SYNAPSES_PER_NODE: u32 = 7000;
const LRU_CAPACITY: usize = 1000;
const INITIAL_WEIGHT: f32 = 0.01;
const DEFAULT_THRESHOLD: f32 = 0.2;
const PRUNE_RATIO: f32 = 0.3;
const TEMPORAL_WINDOW_SIZE: usize = 5;
const MAX_SPREAD_DEPTH: u8 = 4;

const CACHE_RECOMPUTE_ACCESS_INTERVAL: u32 = 500;
const DEFAULT_CACHE_POLICY: &str = "pinned_lru";
const DEFAULT_CACHE_RAM_FRACTION: f32 = 0.25;
const DEFAULT_CACHE_RAM_MIN_MB: u64 = 256;
const DEFAULT_CACHE_RAM_MAX_MB: u64 = 1536;
const DEFAULT_CACHE_PIN_FRACTION: f32 = 0.35;
const DEFAULT_INNATE_REGISTRY_VERSION: u32 = 1;
const DEFAULT_ASYNC_RAM_WARN_MB: u64 = 1024;
const DEFAULT_ASYNC_RAM_CRITICAL_MB: u64 = 1536;
const DEFAULT_ASYNC_COALESCE_WINDOW_MS: u64 = 300;
const DEFAULT_ASYNC_WRITE_THROTTLE_PER_SEC: u32 = 5000;

#[derive(Clone, Debug)]
struct AsyncPolicy {
    ram_warn_mb: u64,
    ram_critical_mb: u64,
    coalesce_window_ms: u64,
    write_throttle_per_sec: u32,
}

#[derive(Clone, Debug)]
struct AsyncRuntimeState {
    enabled: bool,
    ingress_paused: bool,
    shard_count: usize,
    global_queue_len: u64,
    dropped_total: u64,
    coalesced_total: u64,
    hop_total: u64,
    processed_total: u64,
    processed_per_sec: f64,
    last_rate_ts_ms: u64,
    last_rate_processed_total: u64,
    guard_mode: String,
    per_shard_queue_len: Vec<u64>,
    per_shard_processed: Vec<u64>,
    policy: AsyncPolicy,
}

#[derive(Clone, Debug)]
struct NodeMeta {
    node_id: u64,
    synapse_count: u32,
    synapse_offset: u64,
    threshold: f32,
    checksum: u32,
}

#[derive(Clone, Debug)]
struct Synapse {
    receiver_id: u64,
    weight: f32,
}

#[derive(Clone, Debug)]
struct AsyncSynapse {
    receiver_id: u64,
    weight: f32,
}

#[derive(Debug)]
struct AsyncShared {
    shard_count: usize,
    adjacency: HashMap<u64, Vec<AsyncSynapse>>,
    threshold: HashMap<u64, f32>,
    activation: HashMap<u64, f32>,
    ingress_paused: bool,
    global_queue_len: u64,
    per_shard_queue_len: Vec<u64>,
    processed_total: u64,
    processed_per_sec: f64,
    last_rate_ts_ms: u64,
    last_rate_processed_total: u64,
    dropped_total: u64,
    coalesced_total: u64,
    hop_total: u64,
    guard_mode: String,
    per_shard_processed: Vec<u64>,
}

enum ShardCommand {
    Stimulus {
        node_id: u64,
        strength: f32,
        source: String,
        origin_tick: u64,
        reply: oneshot::Sender<bool>,
    },
    Hop {
        node_id: u64,
        strength: f32,
        origin_tick: u64,
        source_shard: usize,
    },
    UpdateEdge {
        sender: u64,
        receiver: u64,
        weight: f32,
        reply: oneshot::Sender<bool>,
    },
    Flush {
        reply: oneshot::Sender<()>,
    },
    Stop,
}

struct AsyncActorRuntime {
    rt: TokioRuntime,
    shard_txs: Vec<mpsc::UnboundedSender<ShardCommand>>,
    shared: Arc<TokioMutex<AsyncShared>>,
    global_tick: Arc<AtomicU64>,
}

#[derive(Clone, Debug)]
struct DeltaEntry {
    sender_id: u64,
    receiver_id: u64,
    weight: f32,
    timestamp: u32,
}

#[pyclass]
struct RagpEngine {
    storage_dir: PathBuf,
    base_path: PathBuf,
    delta_path: PathBuf,
    node_index: HashMap<u64, NodeMeta>,
    delta_index: HashMap<u64, HashMap<u64, (f32, u32)>>,
    activation: HashMap<u64, f32>,
    temporal_window: VecDeque<(u64, f32, u32)>,
    tick: u32,

    // Hybrid cache: pinned + LRU
    base_cache: LruCache<u64, Vec<Synapse>>,
    pinned_cache: HashMap<u64, Vec<Synapse>>,
    pinned_set: HashSet<u64>,
    access_count: HashMap<u64, u32>,
    access_since_recompute: u32,

    // Cache policy config
    cache_policy: String,
    cache_ram_fraction: f32,
    cache_ram_min_mb: u64,
    cache_ram_max_mb: u64,
    cache_pin_fraction: f32,

    // Computed cache budgets/metrics
    cache_budget_bytes: u64,
    pinned_budget_bytes: u64,
    lru_budget_bytes: u64,
    cache_bytes_est: u64,
    pinned_bytes_est: u64,
    lru_bytes_est: u64,
    registry_version: u32,
    loaded_registry_version: u32,
    async_state: AsyncRuntimeState,
    async_runtime: Option<AsyncActorRuntime>,
}

impl RagpEngine {
    fn crc32(data: &[u8]) -> u32 {
        crc32fast::hash(data)
    }

    fn env_f32(key: &str, default: f32) -> f32 {
        env::var(key)
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(default)
    }

    fn env_u64(key: &str, default: u64) -> u64 {
        env::var(key)
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(default)
    }

    fn env_u32(key: &str, default: u32) -> u32 {
        env::var(key)
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(default)
    }

    fn default_shard_count() -> usize {
        let cpus = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        let half = (cpus / 2).max(2);
        half
    }

    fn default_async_state() -> AsyncRuntimeState {
        AsyncRuntimeState {
            enabled: false,
            ingress_paused: false,
            shard_count: Self::default_shard_count(),
            global_queue_len: 0,
            dropped_total: 0,
            coalesced_total: 0,
            hop_total: 0,
            processed_total: 0,
            processed_per_sec: 0.0,
            last_rate_ts_ms: 0,
            last_rate_processed_total: 0,
            guard_mode: "normal".to_string(),
            per_shard_queue_len: vec![0; Self::default_shard_count()],
            per_shard_processed: vec![0; Self::default_shard_count()],
            policy: AsyncPolicy {
                ram_warn_mb: DEFAULT_ASYNC_RAM_WARN_MB,
                ram_critical_mb: DEFAULT_ASYNC_RAM_CRITICAL_MB,
                coalesce_window_ms: DEFAULT_ASYNC_COALESCE_WINDOW_MS,
                write_throttle_per_sec: DEFAULT_ASYNC_WRITE_THROTTLE_PER_SEC,
            },
        }
    }

    fn owner_shard(&self, sender: u64) -> usize {
        if self.async_state.shard_count == 0 {
            return 0;
        }
        (sender as usize) % self.async_state.shard_count
    }

    fn refresh_async_guard_mode(&mut self) {
        let mut sys = System::new();
        sys.refresh_memory();
        let avail_bytes = Self::normalize_available_bytes(sys.available_memory());
        let avail_mb = avail_bytes / (1024 * 1024);

        self.async_state.guard_mode = if avail_mb <= self.async_state.policy.ram_critical_mb {
            "critical".to_string()
        } else if avail_mb <= self.async_state.policy.ram_warn_mb {
            "warn".to_string()
        } else {
            "normal".to_string()
        };
    }

    fn now_ms() -> u64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(d) => d.as_millis() as u64,
            Err(_) => 0,
        }
    }

    fn refresh_async_processed_rate(&mut self) {
        let now = Self::now_ms();
        if self.async_state.last_rate_ts_ms == 0 {
            self.async_state.last_rate_ts_ms = now;
            self.async_state.last_rate_processed_total = self.async_state.processed_total;
            self.async_state.processed_per_sec = 0.0;
            return;
        }

        let dt_ms = now.saturating_sub(self.async_state.last_rate_ts_ms);
        if dt_ms < 200 {
            return;
        }
        let dp = self
            .async_state
            .processed_total
            .saturating_sub(self.async_state.last_rate_processed_total);
        self.async_state.processed_per_sec = (dp as f64) / (dt_ms as f64 / 1000.0);
        self.async_state.last_rate_ts_ms = now;
        self.async_state.last_rate_processed_total = self.async_state.processed_total;
    }

    fn build_async_snapshot(&mut self) -> (HashMap<u64, Vec<AsyncSynapse>>, HashMap<u64, f32>) {
        let mut senders: Vec<u64> = self.node_index.keys().copied().collect();
        senders.sort_unstable();

        let mut adjacency: HashMap<u64, Vec<AsyncSynapse>> = HashMap::new();
        for sender in senders {
            let conns = self.get_connections_internal(sender);
            let syns: Vec<AsyncSynapse> = conns
                .into_iter()
                .map(|(receiver_id, weight)| AsyncSynapse { receiver_id, weight })
                .collect();
            adjacency.insert(sender, syns);
        }

        let mut thresholds: HashMap<u64, f32> = HashMap::new();
        for (node, meta) in &self.node_index {
            thresholds.insert(*node, meta.threshold);
        }
        (adjacency, thresholds)
    }

    fn sync_async_state_from_shared(&mut self) {
        let Some(ar) = self.async_runtime.as_ref() else {
            return;
        };
        let snapshot = ar.rt.block_on(async {
            let s = ar.shared.lock().await;
            (
                s.ingress_paused,
                s.global_queue_len,
                s.processed_total,
                s.processed_per_sec,
                s.dropped_total,
                s.coalesced_total,
                s.hop_total,
                s.guard_mode.clone(),
                s.per_shard_queue_len.clone(),
                s.per_shard_processed.clone(),
            )
        });

        self.async_state.ingress_paused = snapshot.0;
        self.async_state.global_queue_len = snapshot.1;
        self.async_state.processed_total = snapshot.2;
        self.async_state.processed_per_sec = snapshot.3;
        self.async_state.dropped_total = snapshot.4;
        self.async_state.coalesced_total = snapshot.5;
        self.async_state.hop_total = snapshot.6;
        self.async_state.guard_mode = snapshot.7;
        self.async_state.per_shard_queue_len = snapshot.8;
        self.async_state.per_shard_processed = snapshot.9;
    }

    fn env_policy(key: &str, default: &str) -> String {
        let policy = env::var(key).unwrap_or_else(|_| default.to_string());
        let normalized = policy.trim().to_ascii_lowercase();
        if normalized == "pinned_lru" || normalized == "lru" {
            normalized
        } else {
            default.to_string()
        }
    }

    fn clamp_f32(v: f32, lo: f32, hi: f32) -> f32 {
        v.max(lo).min(hi)
    }

    fn normalize_available_bytes(raw: u64) -> u64 {
        // Sysinfo versions differ (bytes vs KiB). Normalize to bytes heuristically.
        if raw < (1_u64 << 31) {
            raw.saturating_mul(1024)
        } else {
            raw
        }
    }

    fn node_cache_bytes_from_len(len: usize) -> u64 {
        (len as u64).saturating_mul(SYNAPSE_SIZE).saturating_add(64)
    }

    fn chunk_start_for_sender(sender: u64) -> u64 {
        if sender == 0 {
            return 1;
        }
        ((sender - 1) / CHUNK_SPAN) * CHUNK_SPAN + 1
    }

    fn chunk_end_from_start(start: u64) -> u64 {
        start.saturating_add(CHUNK_SPAN - 1)
    }

    fn chunk_file_name(start: u64) -> String {
        let end = Self::chunk_end_from_start(start);
        format!("base_{:06}_{:06}.bin", start, end)
    }

    fn chunk_file_path(&self, start: u64) -> PathBuf {
        self.storage_dir.join(Self::chunk_file_name(start))
    }

    fn encode_chunk_offset(chunk_start: u64, local_offset: u32) -> u64 {
        OFFSET_CHUNK_FLAG | (chunk_start << 32) | u64::from(local_offset)
    }

    fn is_chunk_offset(encoded: u64) -> bool {
        (encoded & OFFSET_CHUNK_FLAG) != 0
    }

    fn decode_chunk_offset(encoded: u64) -> (u64, u64) {
        let chunk_start = (encoded & !OFFSET_CHUNK_FLAG) >> 32;
        let local_offset = encoded & 0xFFFF_FFFF;
        (chunk_start, local_offset)
    }

    fn chunk_file_starts(&self) -> Vec<u64> {
        let mut out: Vec<u64> = Vec::new();
        let Ok(entries) = fs::read_dir(&self.storage_dir) else {
            return out;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with("base_") || !name.ends_with(".bin") {
                continue;
            }
            let raw = name.trim_end_matches(".bin");
            let parts: Vec<&str> = raw.split('_').collect();
            if parts.len() != 3 {
                continue;
            }
            if let Ok(start) = parts[1].parse::<u64>() {
                out.push(start);
            }
        }
        out.sort_unstable();
        out
    }

    fn clear_chunk_files(&self) {
        let starts = self.chunk_file_starts();
        for start in starts {
            let _ = fs::remove_file(self.chunk_file_path(start));
        }
    }

    fn has_chunk_files(&self) -> bool {
        !self.chunk_file_starts().is_empty()
    }

    fn refresh_cache_budget(&mut self) {
        let mut sys = System::new();
        sys.refresh_memory();

        let avail_raw = sys.available_memory();
        let avail_bytes = Self::normalize_available_bytes(avail_raw);

        let fraction = Self::clamp_f32(self.cache_ram_fraction, 0.01, 0.90);
        let min_bytes = self.cache_ram_min_mb.saturating_mul(1024 * 1024);
        let max_bytes = self.cache_ram_max_mb.saturating_mul(1024 * 1024).max(min_bytes);

        let mut target = ((avail_bytes as f64) * (fraction as f64)) as u64;
        if target < min_bytes {
            target = min_bytes;
        }
        if target > max_bytes {
            target = max_bytes;
        }

        self.cache_budget_bytes = target;
        if self.cache_policy == "pinned_lru" {
            let pin_fraction = Self::clamp_f32(self.cache_pin_fraction, 0.05, 0.90);
            self.pinned_budget_bytes = ((target as f64) * (pin_fraction as f64)) as u64;
            self.lru_budget_bytes = target.saturating_sub(self.pinned_budget_bytes);
        } else {
            self.pinned_budget_bytes = 0;
            self.lru_budget_bytes = target;
        }

        self.enforce_cache_budget();
    }

    fn recount_cache_bytes(&mut self) {
        self.pinned_bytes_est = self
            .pinned_cache
            .values()
            .map(|v| Self::node_cache_bytes_from_len(v.len()))
            .sum();
        self.lru_bytes_est = self
            .base_cache
            .iter()
            .map(|(_, v)| Self::node_cache_bytes_from_len(v.len()))
            .sum();
        self.cache_bytes_est = self.pinned_bytes_est.saturating_add(self.lru_bytes_est);
    }
    fn pinned_score_from_synapses(&self, node_id: u64, synapses: &[Synapse], max_access: f32) -> f32 {
        let max_weight = synapses
            .iter()
            .fold(0.0_f32, |acc, s| if s.weight > acc { s.weight } else { acc });
        let access = self.access_count.get(&node_id).copied().unwrap_or(0) as f32;
        let access_norm = if max_access <= 0.0 { 0.0 } else { access / max_access };
        0.6 * max_weight + 0.4 * access_norm
    }

    fn lowest_scored_pinned_cached(&self) -> Option<u64> {
        let max_access = self.access_count.values().copied().max().unwrap_or(1) as f32;
        let mut worst: Option<(u64, f32)> = None;
        for (node_id, synapses) in &self.pinned_cache {
            let score = self.pinned_score_from_synapses(*node_id, synapses, max_access);
            match worst {
                Some((_, wscore)) if score >= wscore => {}
                _ => worst = Some((*node_id, score)),
            }
        }
        worst.map(|(id, _)| id)
    }

    fn enforce_cache_budget(&mut self) {
        self.recount_cache_bytes();

        while self.lru_bytes_est > self.lru_budget_bytes {
            if self.base_cache.pop_lru().is_none() {
                break;
            }
            self.recount_cache_bytes();
        }

        if self.cache_policy == "pinned_lru" {
            while self.pinned_bytes_est > self.pinned_budget_bytes {
                let Some(victim) = self.lowest_scored_pinned_cached() else {
                    break;
                };
                self.pinned_cache.remove(&victim);
                self.pinned_set.remove(&victim);
                self.recount_cache_bytes();
            }
        }

        while self.cache_bytes_est > self.cache_budget_bytes {
            if self.base_cache.pop_lru().is_some() {
                self.recount_cache_bytes();
                continue;
            }
            let Some(victim) = self.lowest_scored_pinned_cached() else {
                break;
            };
            self.pinned_cache.remove(&victim);
            self.pinned_set.remove(&victim);
            self.recount_cache_bytes();
        }
    }

    fn get_cached_or_load_base(&mut self, sender: u64) -> Vec<Synapse> {
        if self.cache_policy == "pinned_lru" {
            if let Some(v) = self.pinned_cache.get(&sender) {
                return v.clone();
            }
        }

        if let Some(v) = self.base_cache.get(&sender) {
            return v.clone();
        }

        let loaded = self.load_from_base(sender);
        if self.cache_policy == "pinned_lru" && self.pinned_set.contains(&sender) {
            self.pinned_cache.insert(sender, loaded.clone());
        } else {
            self.base_cache.put(sender, loaded.clone());
        }
        self.enforce_cache_budget();
        loaded
    }

    fn invalidate_sender_cache(&mut self, sender: u64) {
        self.pinned_cache.remove(&sender);
        self.base_cache.pop(&sender);
        self.enforce_cache_budget();
    }

    fn record_access(&mut self, sender: u64) {
        let entry = self.access_count.entry(sender).or_insert(0);
        *entry = entry.saturating_add(1);
        self.access_since_recompute = self.access_since_recompute.saturating_add(1);
        if self.access_since_recompute >= CACHE_RECOMPUTE_ACCESS_INTERVAL {
            self.access_since_recompute = 0;
            self.refresh_cache_budget();
            self.recompute_pinned_set(false);
        }
    }

    fn recompute_pinned_set(&mut self, eager_warm: bool) {
        if self.cache_policy != "pinned_lru" {
            self.pinned_set.clear();
            self.pinned_cache.clear();
            self.enforce_cache_budget();
            return;
        }

        let max_access = self.access_count.values().copied().max().unwrap_or(1) as f32;
        let node_ids: Vec<u64> = self.node_index.keys().copied().collect();
        let mut scored: Vec<(u64, f32, u64)> = Vec::with_capacity(node_ids.len());

        for node_id in node_ids {
            let synapses = if let Some(v) = self.pinned_cache.get(&node_id) {
                v.clone()
            } else if let Some(v) = self.base_cache.get(&node_id) {
                v.clone()
            } else {
                self.load_from_base(node_id)
            };
            let score = self.pinned_score_from_synapses(node_id, &synapses, max_access);
            let est = Self::node_cache_bytes_from_len(synapses.len());
            scored.push((node_id, score, est));
        }

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let mut new_pinned: HashSet<u64> = HashSet::new();
        let mut used: u64 = 0;
        for (node_id, _, est) in scored {
            let next = used.saturating_add(est);
            if !new_pinned.is_empty() && next > self.pinned_budget_bytes {
                continue;
            }
            if new_pinned.is_empty() && est > self.pinned_budget_bytes {
                new_pinned.insert(node_id);
                used = est;
                continue;
            }
            if next <= self.pinned_budget_bytes {
                new_pinned.insert(node_id);
                used = next;
            }
        }
        self.pinned_set = new_pinned;

        let old_pinned_keys: Vec<u64> = self.pinned_cache.keys().copied().collect();
        for key in old_pinned_keys {
            if !self.pinned_set.contains(&key) {
                if let Some(v) = self.pinned_cache.remove(&key) {
                    self.base_cache.put(key, v);
                }
            }
        }

        let keys_to_promote: Vec<u64> = self.pinned_set.iter().copied().collect();
        for key in keys_to_promote {
            if self.pinned_cache.contains_key(&key) {
                continue;
            }
            if let Some(v) = self.base_cache.pop(&key) {
                self.pinned_cache.insert(key, v);
                continue;
            }
            if eager_warm {
                let loaded = self.load_from_base(key);
                self.pinned_cache.insert(key, loaded);
            }
        }

        self.enforce_cache_budget();
    }
    fn load_node_index(&mut self) {
        self.node_index.clear();
        self.loaded_registry_version = DEFAULT_INNATE_REGISTRY_VERSION;
        let mut f = match File::open(&self.base_path) {
            Ok(file) => file,
            Err(_) => return,
        };

        let mut header = [0_u8; BASE_HEADER_SIZE as usize];
        if f.read_exact(&mut header).is_err() {
            return;
        }

        let magic = u32::from_le_bytes(header[0..4].try_into().unwrap());
        if magic != MAGIC_BASE {
            return;
        }
        let version = u16::from_le_bytes(header[4..6].try_into().unwrap());
        if version != VERSION {
            return;
        }
        let reg = u32::from_le_bytes(header[10..14].try_into().unwrap());
        if reg > 0 {
            self.loaded_registry_version = reg;
        }

        let node_count = u32::from_le_bytes(header[6..10].try_into().unwrap());
        for _ in 0..node_count {
            let mut rec = [0_u8; NODE_INDEX_SIZE as usize];
            if f.read_exact(&mut rec).is_err() {
                break;
            }
            let node_id = u64::from_le_bytes(rec[0..8].try_into().unwrap());
            let synapse_count = u32::from_le_bytes(rec[8..12].try_into().unwrap());
            let synapse_offset = u64::from_le_bytes(rec[12..20].try_into().unwrap());
            let threshold = f32::from_le_bytes(rec[20..24].try_into().unwrap());
            let checksum = u32::from_le_bytes(rec[24..28].try_into().unwrap());
            self.node_index.insert(
                node_id,
                NodeMeta {
                    node_id,
                    synapse_count,
                    synapse_offset,
                    threshold,
                    checksum,
                },
            );
        }
    }

    fn load_delta_index(&mut self) {
        let mut f = match File::open(&self.delta_path) {
            Ok(file) => file,
            Err(_) => return,
        };

        let mut header = [0_u8; DELTA_HEADER_SIZE as usize];
        if f.read_exact(&mut header).is_err() {
            return;
        }
        let magic = u32::from_le_bytes(header[0..4].try_into().unwrap());
        if magic != MAGIC_DELTA {
            return;
        }
        let version = u16::from_le_bytes(header[4..6].try_into().unwrap());
        if version != VERSION {
            return;
        }
        let delta_registry_version = u16::from_le_bytes(header[6..8].try_into().unwrap()) as u32;
        if delta_registry_version != self.registry_version {
            return;
        }

        let file_size = match f.metadata() {
            Ok(m) => m.len(),
            Err(_) => return,
        };
        if file_size <= DELTA_HEADER_SIZE {
            return;
        }

        let entry_count = (file_size - DELTA_HEADER_SIZE) / DELTA_ENTRY_SIZE;
        let mut max_ts = self.tick;
        for _ in 0..entry_count {
            let mut raw = [0_u8; DELTA_ENTRY_SIZE as usize];
            if f.read_exact(&mut raw).is_err() {
                break;
            }

            let payload = &raw[0..24];
            let checksum = u32::from_le_bytes(raw[24..28].try_into().unwrap());
            if Self::crc32(payload) != checksum {
                continue;
            }

            let sender = u64::from_le_bytes(raw[0..8].try_into().unwrap());
            let receiver = u64::from_le_bytes(raw[8..16].try_into().unwrap());
            let weight = f32::from_le_bytes(raw[16..20].try_into().unwrap());
            let timestamp = u32::from_le_bytes(raw[20..24].try_into().unwrap());

            if !self.node_index.contains_key(&sender) || !self.node_index.contains_key(&receiver) {
                continue;
            }

            let sender_map = self.delta_index.entry(sender).or_default();
            match sender_map.get(&receiver) {
                Some((_, old_ts)) if *old_ts > timestamp => {}
                _ => {
                    sender_map.insert(receiver, (weight, timestamp));
                }
            }

            let next_tick = timestamp.saturating_add(1);
            if next_tick > max_ts {
                max_ts = next_tick;
            }
        }
        if max_ts > self.tick {
            self.tick = max_ts;
        }
    }

    fn read_synapses_at(&self, offset: u64, count: u32) -> Vec<Synapse> {
        if offset == u64::MAX || count == 0 {
            return Vec::new();
        }
        let mut f = if Self::is_chunk_offset(offset) {
            let (chunk_start, local_offset) = Self::decode_chunk_offset(offset);
            let path = self.chunk_file_path(chunk_start);
            let Ok(mut file) = File::open(path) else {
                return Vec::new();
            };
            if file.seek(SeekFrom::Start(local_offset)).is_err() {
                return Vec::new();
            }
            file
        } else {
            // Legacy monolithic format fallback.
            let Ok(mut file) = File::open(&self.base_path) else {
                return Vec::new();
            };
            if file.seek(SeekFrom::Start(offset)).is_err() {
                return Vec::new();
            }
            file
        };

        let mut synapses = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let mut buf = [0_u8; SYNAPSE_SIZE as usize];
            if f.read_exact(&mut buf).is_err() {
                break;
            }
            let receiver_id = u64::from_le_bytes(buf[0..8].try_into().unwrap());
            let weight = f32::from_le_bytes(buf[8..12].try_into().unwrap());
            synapses.push(Synapse { receiver_id, weight });
        }
        synapses
    }

    fn load_from_base(&mut self, sender: u64) -> Vec<Synapse> {
        let (offset, count) = match self.node_index.get(&sender) {
            Some(meta) => (meta.synapse_offset, meta.synapse_count),
            None => return Vec::new(),
        };
        self.read_synapses_at(offset, count)
    }

    fn append_delta_entry(&self, entry: &DeltaEntry) {
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.delta_path)
            .expect("Gagal membuka delta.bin");

        let mut payload = [0_u8; 24];
        payload[0..8].copy_from_slice(&entry.sender_id.to_le_bytes());
        payload[8..16].copy_from_slice(&entry.receiver_id.to_le_bytes());
        payload[16..20].copy_from_slice(&entry.weight.to_le_bytes());
        payload[20..24].copy_from_slice(&entry.timestamp.to_le_bytes());
        let checksum = Self::crc32(&payload);

        f.write_all(&payload).unwrap();
        f.write_all(&checksum.to_le_bytes()).unwrap();
    }

    fn init_delta_if_needed(&self) {
        if self.delta_path.exists() {
            return;
        }
        let mut f = File::create(&self.delta_path).expect("Gagal membuat delta.bin");
        f.write_all(&MAGIC_DELTA.to_le_bytes()).unwrap();
        f.write_all(&VERSION.to_le_bytes()).unwrap();
        let reg = self.registry_version.min(u16::MAX as u32) as u16;
        f.write_all(&reg.to_le_bytes()).unwrap();
    }

    fn synapse_count_for(&self, sender: u64) -> u32 {
        let base = self.node_index.get(&sender).map_or(0, |m| m.synapse_count);
        let delta = self.delta_index.get(&sender).map_or(0, |m| m.len() as u32);
        base.saturating_add(delta)
    }

    fn write_base_manifest_and_chunks(&mut self, all_data: &[(u64, Vec<Synapse>)]) {
        self.clear_chunk_files();

        let mut chunk_buffers: HashMap<u64, Vec<u8>> = HashMap::new();
        let mut records: Vec<(u64, u32, u64, f32, u32)> = Vec::new();

        for (node_id, synapses) in all_data {
            let threshold = self
                .node_index
                .get(node_id)
                .map_or(DEFAULT_THRESHOLD, |m| m.threshold);

            if synapses.is_empty() {
                records.push((*node_id, 0, u64::MAX, threshold, 0));
                continue;
            }

            let chunk_start = Self::chunk_start_for_sender(*node_id);
            let chunk_buf = chunk_buffers.entry(chunk_start).or_default();
            let local_offset = chunk_buf.len() as u64;
            if local_offset > u32::MAX as u64 {
                panic!("Chunk offset overflow for sender {}", node_id);
            }

            let mut syn_bytes: Vec<u8> = Vec::with_capacity(synapses.len() * SYNAPSE_SIZE as usize);
            for s in synapses {
                syn_bytes.extend_from_slice(&s.receiver_id.to_le_bytes());
                syn_bytes.extend_from_slice(&s.weight.to_le_bytes());
            }
            let checksum = Self::crc32(&syn_bytes);
            chunk_buf.extend_from_slice(&syn_bytes);

            let encoded_offset = Self::encode_chunk_offset(chunk_start, local_offset as u32);
            records.push((*node_id, synapses.len() as u32, encoded_offset, threshold, checksum));
        }

        records.sort_by_key(|(node_id, _, _, _, _)| *node_id);
        let node_count = records.len() as u32;

        let mut manifest = File::create(&self.base_path).expect("Gagal menulis base manifest");
        manifest.write_all(&MAGIC_BASE.to_le_bytes()).unwrap();
        manifest.write_all(&VERSION.to_le_bytes()).unwrap();
        manifest.write_all(&node_count.to_le_bytes()).unwrap();
        manifest.write_all(&self.registry_version.to_le_bytes()).unwrap();
        for (node_id, count, offset, threshold, checksum) in &records {
            manifest.write_all(&node_id.to_le_bytes()).unwrap();
            manifest.write_all(&count.to_le_bytes()).unwrap();
            manifest.write_all(&offset.to_le_bytes()).unwrap();
            manifest.write_all(&threshold.to_le_bytes()).unwrap();
            manifest.write_all(&checksum.to_le_bytes()).unwrap();
            manifest.write_all(&0_u32.to_le_bytes()).unwrap();
        }

        let mut chunk_starts: Vec<u64> = chunk_buffers.keys().copied().collect();
        chunk_starts.sort_unstable();
        for start in chunk_starts {
            let path = self.chunk_file_path(start);
            let mut f = File::create(path).expect("Gagal menulis chunk file");
            if let Some(buf) = chunk_buffers.get(&start) {
                f.write_all(buf).unwrap();
            }
        }

        for (node_id, count, offset, threshold, checksum) in records {
            if let Some(meta) = self.node_index.get_mut(&node_id) {
                meta.synapse_count = count;
                meta.synapse_offset = offset;
                meta.threshold = threshold;
                meta.checksum = checksum;
            }
        }
    }

    fn maybe_migrate_legacy_base_to_chunks(&mut self) {
        if self.node_index.is_empty() || self.has_chunk_files() {
            return;
        }

        let mut has_legacy_offsets = false;
        for meta in self.node_index.values() {
            if meta.synapse_count > 0
                && meta.synapse_offset != u64::MAX
                && !Self::is_chunk_offset(meta.synapse_offset)
            {
                has_legacy_offsets = true;
                break;
            }
        }
        if !has_legacy_offsets {
            return;
        }

        let mut node_ids: Vec<u64> = self.node_index.keys().copied().collect();
        node_ids.sort_unstable();
        let mut all_data: Vec<(u64, Vec<Synapse>)> = Vec::with_capacity(node_ids.len());
        for node_id in node_ids {
            all_data.push((node_id, self.load_from_base(node_id)));
        }
        self.write_base_manifest_and_chunks(&all_data);
        println!("[Migrasi] base.bin lama dimigrasikan ke chunk range");
    }
    fn rebuild_base_bin(&mut self) {
        let node_ids: Vec<u64> = self.node_index.keys().copied().collect();
        let mut all_data: Vec<(u64, Vec<Synapse>)> = Vec::new();

        for node_id in &node_ids {
            let mut merged = self.load_from_base(*node_id);
            if let Some(delta) = self.delta_index.get(node_id) {
                for (receiver, (weight, _)) in delta {
                    if let Some(existing) = merged.iter_mut().find(|s| s.receiver_id == *receiver) {
                        existing.weight = *weight;
                    } else {
                        merged.push(Synapse {
                            receiver_id: *receiver,
                            weight: *weight,
                        });
                    }
                }
            }

            if !merged.is_empty() {
                let avg = merged.iter().map(|s| s.weight).sum::<f32>() / merged.len() as f32;
                let threshold = avg * PRUNE_RATIO;
                merged.retain(|s| s.weight >= threshold);
                merged.sort_by(|a, b| b.weight.partial_cmp(&a.weight).unwrap_or(std::cmp::Ordering::Equal));
            }

            all_data.push((*node_id, merged));
        }

        all_data.sort_by_key(|(node_id, _)| *node_id);
        self.write_base_manifest_and_chunks(&all_data);
    }

    fn migrate_innate_registry(&mut self, node_ids: Vec<u64>) -> (u32, u32) {
        let mut sorted_ids = node_ids;
        sorted_ids.sort_unstable();
        sorted_ids.dedup();
        if sorted_ids.is_empty() {
            return (0, 0);
        }

        if self.node_index.is_empty() {
            self.init_node_pool(sorted_ids);
            self.loaded_registry_version = self.registry_version;
            return (0, 0);
        }

        let target_set: HashSet<u64> = sorted_ids.iter().copied().collect();
        let old_ids: Vec<u64> = self.node_index.keys().copied().collect();
        let old_set: HashSet<u64> = old_ids.iter().copied().collect();

        let mut old_data: HashMap<u64, Vec<Synapse>> = HashMap::new();
        for sender in &old_ids {
            let mut merged = self.load_from_base(*sender);
            if let Some(delta) = self.delta_index.get(sender) {
                for (receiver, (weight, _)) in delta {
                    if let Some(existing) = merged.iter_mut().find(|s| s.receiver_id == *receiver) {
                        existing.weight = *weight;
                    } else {
                        merged.push(Synapse {
                            receiver_id: *receiver,
                            weight: *weight,
                        });
                    }
                }
            }
            old_data.insert(*sender, merged);
        }

        self.node_index.clear();
        for id in &sorted_ids {
            self.node_index.insert(
                *id,
                NodeMeta {
                    node_id: *id,
                    synapse_count: 0,
                    synapse_offset: u64::MAX,
                    threshold: DEFAULT_THRESHOLD,
                    checksum: 0,
                },
            );
        }

        let mut all_data: Vec<(u64, Vec<Synapse>)> = Vec::with_capacity(sorted_ids.len());
        for id in &sorted_ids {
            let mut syns = old_data.remove(id).unwrap_or_default();
            syns.retain(|s| target_set.contains(&s.receiver_id));
            syns.sort_by(|a, b| b.weight.partial_cmp(&a.weight).unwrap_or(std::cmp::Ordering::Equal));
            all_data.push((*id, syns));
        }
        self.write_base_manifest_and_chunks(&all_data);

        let removed_nodes = old_set.difference(&target_set).count() as u32;
        let added_nodes = target_set.difference(&old_set).count() as u32;

        self.delta_index.clear();
        self.activation.clear();
        self.temporal_window.clear();
        self.base_cache.clear();
        self.pinned_cache.clear();
        self.pinned_set.clear();
        self.access_count.clear();
        self.access_since_recompute = 0;
        self.reset_delta_file();
        self.loaded_registry_version = self.registry_version;
        self.refresh_cache_budget();
        self.recompute_pinned_set(true);

        (added_nodes, removed_nodes)
    }

    fn ensure_innate_registry_internal(&mut self, node_ids: Vec<u64>) -> (bool, u32, u32) {
        let mut sorted_ids = node_ids;
        sorted_ids.sort_unstable();
        sorted_ids.dedup();
        if sorted_ids.is_empty() {
            return (false, 0, 0);
        }

        let mut current_ids: Vec<u64> = self.node_index.keys().copied().collect();
        current_ids.sort_unstable();
        let needs_migrate =
            self.node_index.is_empty()
                || self.loaded_registry_version != self.registry_version
                || current_ids != sorted_ids;
        if !needs_migrate {
            return (false, 0, 0);
        }

        let (added, removed) = self.migrate_innate_registry(sorted_ids);
        (true, added, removed)
    }

    fn strict_check_node(&self, node_id: u64, role: &str) -> PyResult<()> {
        if self.node_index.contains_key(&node_id) {
            Ok(())
        } else {
            Err(PyValueError::new_err(format!(
                "Unknown node for {}: {}. Node must be registered in innate registry.",
                role, node_id
            )))
        }
    }

    fn get_connections_internal(&mut self, sender: u64) -> Vec<(u64, f32)> {
        if !self.node_index.contains_key(&sender) {
            return Vec::new();
        }

        self.record_access(sender);
        let base_synapses = self.get_cached_or_load_base(sender);

        let mut merged: HashMap<u64, f32> = HashMap::new();
        for s in base_synapses {
            merged.insert(s.receiver_id, s.weight);
        }
        if let Some(delta) = self.delta_index.get(&sender) {
            for (receiver, (weight, _)) in delta {
                merged.insert(*receiver, *weight);
            }
        }
        merged.into_iter().collect()
    }

    fn reset_delta_file(&self) {
        let mut f = File::create(&self.delta_path).expect("Gagal reset delta.bin");
        f.write_all(&MAGIC_DELTA.to_le_bytes()).unwrap();
        f.write_all(&VERSION.to_le_bytes()).unwrap();
        let reg = self.registry_version.min(u16::MAX as u32) as u16;
        f.write_all(&reg.to_le_bytes()).unwrap();
    }
}

#[pymethods]
impl RagpEngine {
    #[new]
    fn new(storage_dir: String) -> Self {
        let path = PathBuf::from(&storage_dir);
        if !path.exists() {
            std::fs::create_dir_all(&path).expect("Gagal membuat direktori storage");
        }

        let base_path = path.join("base.bin");
        let delta_path = path.join("delta.bin");
        let capacity = NonZeroUsize::new(LRU_CAPACITY).unwrap();

        let mut engine = RagpEngine {
            storage_dir: path.clone(),
            base_path,
            delta_path,
            node_index: HashMap::new(),
            delta_index: HashMap::new(),
            activation: HashMap::new(),
            temporal_window: VecDeque::new(),
            tick: 0,
            base_cache: LruCache::new(capacity),
            pinned_cache: HashMap::new(),
            pinned_set: HashSet::new(),
            access_count: HashMap::new(),
            access_since_recompute: 0,
            cache_policy: Self::env_policy("RAGP_CACHE_POLICY", DEFAULT_CACHE_POLICY),
            cache_ram_fraction: Self::env_f32("RAGP_CACHE_RAM_FRACTION", DEFAULT_CACHE_RAM_FRACTION),
            cache_ram_min_mb: Self::env_u64("RAGP_CACHE_RAM_MIN_MB", DEFAULT_CACHE_RAM_MIN_MB),
            cache_ram_max_mb: Self::env_u64("RAGP_CACHE_RAM_MAX_MB", DEFAULT_CACHE_RAM_MAX_MB),
            cache_pin_fraction: Self::env_f32("RAGP_CACHE_PIN_FRACTION", DEFAULT_CACHE_PIN_FRACTION),
            cache_budget_bytes: 0,
            pinned_budget_bytes: 0,
            lru_budget_bytes: 0,
            cache_bytes_est: 0,
            pinned_bytes_est: 0,
            lru_bytes_est: 0,
            registry_version: Self::env_u32(
                "RAGP_INNATE_REGISTRY_VERSION",
                DEFAULT_INNATE_REGISTRY_VERSION,
            ),
            loaded_registry_version: DEFAULT_INNATE_REGISTRY_VERSION,
            async_state: Self::default_async_state(),
            async_runtime: None,
        };

        engine.load_node_index();
        engine.maybe_migrate_legacy_base_to_chunks();
        engine.load_node_index();
        engine.init_delta_if_needed();
        engine.load_delta_index();
        engine.refresh_cache_budget();
        engine.recompute_pinned_set(true);
        engine
    }
    fn init_node_pool(&mut self, node_ids: Vec<u64>) {
        self.node_index.clear();
        self.delta_index.clear();
        self.activation.clear();
        self.temporal_window.clear();
        self.base_cache.clear();
        self.pinned_cache.clear();
        self.pinned_set.clear();
        self.access_count.clear();
        self.access_since_recompute = 0;
        self.tick = 0;
        self.clear_chunk_files();

        let mut sorted_ids = node_ids;
        sorted_ids.sort_unstable();
        sorted_ids.dedup();

        for id in &sorted_ids {
            self.node_index.insert(
                *id,
                NodeMeta {
                    node_id: *id,
                    synapse_count: 0,
                    synapse_offset: u64::MAX,
                    threshold: DEFAULT_THRESHOLD,
                    checksum: 0,
                },
            );
        }

        let all_data: Vec<(u64, Vec<Synapse>)> = sorted_ids
            .iter()
            .map(|id| (*id, Vec::new()))
            .collect();
        self.write_base_manifest_and_chunks(&all_data);

        self.reset_delta_file();
        self.refresh_cache_budget();
        self.recompute_pinned_set(true);

        println!("[RagpEngine] {} node diinisialisasi (tanpa sinapsis)", self.node_index.len());
    }

    fn ensure_innate_registry(&mut self, node_ids: Vec<u64>) -> String {
        let (migrated, added, removed) = self.ensure_innate_registry_internal(node_ids);
        if migrated {
            format!(
                "migrated=true registry_version={} added_nodes={} removed_nodes={}",
                self.registry_version, added, removed
            )
        } else {
            format!(
                "migrated=false registry_version={} added_nodes=0 removed_nodes=0",
                self.registry_version
            )
        }
    }

    fn start_async_runtime(&mut self, config: Option<&PyAny>) -> PyResult<String> {
        if let Some(obj) = config {
            if !obj.is_none() {
                let cfg = obj.downcast::<PyDict>()?;
                if let Some(v) = cfg.get_item("shard_count")? {
                    if let Ok(sc) = v.extract::<usize>() {
                        self.async_state.shard_count = sc.max(2);
                    }
                }
                if let Some(v) = cfg.get_item("ram_warn_mb")? {
                    if let Ok(x) = v.extract::<u64>() {
                        self.async_state.policy.ram_warn_mb = x.max(128);
                    }
                }
                if let Some(v) = cfg.get_item("ram_critical_mb")? {
                    if let Ok(x) = v.extract::<u64>() {
                        self.async_state.policy.ram_critical_mb =
                            x.max(self.async_state.policy.ram_warn_mb);
                    }
                }
                if let Some(v) = cfg.get_item("coalesce_window_ms")? {
                    if let Ok(x) = v.extract::<u64>() {
                        self.async_state.policy.coalesce_window_ms = x.max(50);
                    }
                }
                if let Some(v) = cfg.get_item("write_throttle_per_sec")? {
                    if let Ok(x) = v.extract::<u32>() {
                        self.async_state.policy.write_throttle_per_sec = x.max(100);
                    }
                }
            }
        }

        if self.async_runtime.is_some() {
            self.sync_async_state_from_shared();
            return Ok(format!(
                "async_on=true shards={} guard_mode={}",
                self.async_state.shard_count, self.async_state.guard_mode
            ));
        }

        self.refresh_async_guard_mode();
        let shard_count = self.async_state.shard_count.max(2);
        let (adjacency, threshold) = self.build_async_snapshot();
        let guard_mode = self.async_state.guard_mode.clone();

        let shared = Arc::new(TokioMutex::new(AsyncShared {
            shard_count,
            adjacency,
            threshold,
            activation: HashMap::new(),
            ingress_paused: false,
            global_queue_len: 0,
            per_shard_queue_len: vec![0; shard_count],
            processed_total: 0,
            processed_per_sec: 0.0,
            last_rate_ts_ms: 0,
            last_rate_processed_total: 0,
            dropped_total: 0,
            coalesced_total: 0,
            hop_total: 0,
            guard_mode,
            per_shard_processed: vec![0; shard_count],
        }));

        let rt = TokioRuntimeBuilder::new_multi_thread()
            .worker_threads(shard_count.max(2))
            .enable_all()
            .build()
            .map_err(|e| PyValueError::new_err(format!("tokio runtime init failed: {e}")))?;

        let mut shard_txs: Vec<mpsc::UnboundedSender<ShardCommand>> = Vec::with_capacity(shard_count);
        let mut shard_rxs: Vec<mpsc::UnboundedReceiver<ShardCommand>> = Vec::with_capacity(shard_count);
        for _ in 0..shard_count {
            let (tx, rx) = mpsc::unbounded_channel();
            shard_txs.push(tx);
            shard_rxs.push(rx);
        }

        for (idx, rx) in shard_rxs.into_iter().enumerate() {
            let shared_cloned = Arc::clone(&shared);
            let senders = shard_txs.clone();
            rt.spawn(async move {
                shard_actor_loop(idx, rx, senders, shared_cloned).await;
            });
        }

        self.async_runtime = Some(AsyncActorRuntime {
            rt,
            shard_txs,
            shared,
            global_tick: Arc::new(AtomicU64::new(self.tick as u64)),
        });

        self.async_state.enabled = true;
        self.async_state.ingress_paused = false;
        self.async_state.global_queue_len = 0;
        self.async_state.processed_total = 0;
        self.async_state.processed_per_sec = 0.0;
        self.async_state.dropped_total = 0;
        self.async_state.coalesced_total = 0;
        self.async_state.hop_total = 0;
        self.async_state.per_shard_queue_len = vec![0; shard_count];
        self.async_state.per_shard_processed = vec![0; shard_count];

        Ok(format!(
            "async_on=true shards={} guard_mode={}",
            shard_count, self.async_state.guard_mode
        ))
    }

    fn stop_async_runtime(&mut self) -> String {
        if let Some(runtime) = self.async_runtime.take() {
            for tx in &runtime.shard_txs {
                let _ = tx.send(ShardCommand::Stop);
            }
            drop(runtime);
        }
        self.async_state.enabled = false;
        self.async_state.ingress_paused = false;
        self.async_state.global_queue_len = 0;
        self.async_state.per_shard_queue_len = vec![0; self.async_state.shard_count];
        "async_on=false".to_string()
    }

    fn submit_stimulus(
        &mut self,
        node_id: u64,
        strength: f32,
        source: Option<String>,
        _ts_ms: Option<u64>,
    ) -> PyResult<bool> {
        self.strict_check_node(node_id, "submit_stimulus(node_id)")?;
        self.refresh_async_guard_mode();
        let Some(runtime) = self.async_runtime.as_ref() else {
            return Err(PyValueError::new_err(
                "async runtime is OFF; call start_async_runtime first",
            ));
        };
        let owner = self.owner_shard(node_id);

        let ingress_ok = runtime.rt.block_on(async {
            let mut s = runtime.shared.lock().await;
            s.guard_mode = self.async_state.guard_mode.clone();
            if s.ingress_paused {
                s.dropped_total = s.dropped_total.saturating_add(1);
                return false;
            }
            if s.guard_mode == "critical" && s.global_queue_len > 20_000 {
                s.dropped_total = s.dropped_total.saturating_add(1);
                return false;
            }
            s.global_queue_len = s.global_queue_len.saturating_add(1);
            if let Some(slot) = s.per_shard_queue_len.get_mut(owner) {
                *slot = slot.saturating_add(1);
            }
            true
        });
        if !ingress_ok {
            self.sync_async_state_from_shared();
            return Ok(false);
        }

        let (tx, rx) = oneshot::channel();
        let cmd = ShardCommand::Stimulus {
            node_id,
            strength: strength.max(0.0).min(1.0),
            source: source.unwrap_or_else(|| "unknown".to_string()),
            origin_tick: runtime.global_tick.fetch_add(1, Ordering::SeqCst),
            reply: tx,
        };
        if runtime.shard_txs[owner].send(cmd).is_err() {
            return Err(PyValueError::new_err("failed to route stimulus to owner shard"));
        }

        let accepted = runtime.rt.block_on(async { rx.await.unwrap_or(false) });
        self.sync_async_state_from_shared();
        Ok(accepted)
    }

    fn submit_stimuli(&mut self, batch: Vec<(u64, f32, String)>) -> PyResult<PyObject> {
        let mut accepted: u64 = 0;
        let mut rejected: u64 = 0;
        let mut coalesced_in_call: u64 = 0;
        let mut grouped: HashMap<(u64, String), f32> = HashMap::new();
        for (node_id, strength, source) in batch {
            let key = (node_id, source);
            if let Some(prev) = grouped.get_mut(&key) {
                if strength > *prev {
                    *prev = strength;
                }
                coalesced_in_call = coalesced_in_call.saturating_add(1);
            } else {
                grouped.insert(key, strength);
            }
        }
        if let Some(runtime) = self.async_runtime.as_ref() {
            runtime.rt.block_on(async {
                let mut s = runtime.shared.lock().await;
                s.coalesced_total = s.coalesced_total.saturating_add(coalesced_in_call);
            });
        }

        let mut grouped_vec: Vec<((u64, String), f32)> = grouped.into_iter().collect();
        grouped_vec.sort_by_key(|((node_id, _), _)| self.owner_shard(*node_id));

        for ((node_id, source), strength) in grouped_vec {
            match self.submit_stimulus(node_id, strength, Some(source), None)? {
                true => accepted = accepted.saturating_add(1),
                false => rejected = rejected.saturating_add(1),
            }
        }
        Python::with_gil(|py| {
            let out = PyDict::new_bound(py);
            out.set_item("ok", true)?;
            out.set_item("accepted", accepted)?;
            out.set_item("rejected", rejected)?;
            out.set_item("coalesced", coalesced_in_call)?;
            Ok(out.to_object(py))
        })
    }

    fn get_async_metrics(&mut self) -> PyResult<PyObject> {
        self.refresh_async_guard_mode();
        self.sync_async_state_from_shared();
        Python::with_gil(|py| {
            let out = PyDict::new_bound(py);
            out.set_item("async_on", self.async_state.enabled)?;
            out.set_item("ingress_paused", self.async_state.ingress_paused)?;
            out.set_item("shard_count", self.async_state.shard_count)?;
            out.set_item("global_queue_len", self.async_state.global_queue_len)?;
            out.set_item("processed_total", self.async_state.processed_total)?;
            out.set_item("processed_per_sec", self.async_state.processed_per_sec)?;
            out.set_item("dropped_total", self.async_state.dropped_total)?;
            out.set_item("coalesced_total", self.async_state.coalesced_total)?;
            out.set_item("hop_total", self.async_state.hop_total)?;
            out.set_item("guard_mode", self.async_state.guard_mode.clone())?;

            let shard_rows = PyDict::new_bound(py);
            for (idx, cnt) in self.async_state.per_shard_processed.iter().enumerate() {
                shard_rows.set_item(idx, *cnt)?;
            }
            out.set_item("per_shard_processed", shard_rows)?;
            let shard_queues = PyDict::new_bound(py);
            for (idx, qlen) in self.async_state.per_shard_queue_len.iter().enumerate() {
                shard_queues.set_item(idx, *qlen)?;
            }
            out.set_item("per_shard_queue_len", shard_queues)?;
            Ok(out.to_object(py))
        })
    }

    fn set_async_policy(
        &mut self,
        ram_warn_mb: Option<u64>,
        ram_critical_mb: Option<u64>,
        coalesce_window_ms: Option<u64>,
        write_throttle_per_sec: Option<u32>,
    ) -> PyResult<PyObject> {
        if let Some(v) = ram_warn_mb {
            self.async_state.policy.ram_warn_mb = v.max(128);
        }
        if let Some(v) = ram_critical_mb {
            self.async_state.policy.ram_critical_mb = v.max(self.async_state.policy.ram_warn_mb);
        }
        if let Some(v) = coalesce_window_ms {
            self.async_state.policy.coalesce_window_ms = v.max(50);
        }
        if let Some(v) = write_throttle_per_sec {
            self.async_state.policy.write_throttle_per_sec = v.max(100);
        }
        self.refresh_async_guard_mode();
        if let Some(runtime) = self.async_runtime.as_ref() {
            runtime.rt.block_on(async {
                let mut s = runtime.shared.lock().await;
                s.guard_mode = self.async_state.guard_mode.clone();
            });
        }
        Python::with_gil(|py| {
            let out = PyDict::new_bound(py);
            out.set_item("ok", true)?;
            out.set_item("ram_warn_mb", self.async_state.policy.ram_warn_mb)?;
            out.set_item("ram_critical_mb", self.async_state.policy.ram_critical_mb)?;
            out.set_item("coalesce_window_ms", self.async_state.policy.coalesce_window_ms)?;
            out.set_item("write_throttle_per_sec", self.async_state.policy.write_throttle_per_sec)?;
            out.set_item("guard_mode", self.async_state.guard_mode.clone())?;
            Ok(out.to_object(py))
        })
    }

    fn get_connections(&mut self, sender: u64) -> PyResult<Vec<(u64, f32)>> {
        self.strict_check_node(sender, "get_connections(sender)")?;
        Ok(self.get_connections_internal(sender))
    }

    fn spread_activation(&mut self, seed_node: u64, seed_strength: f32) -> PyResult<()> {
        self.strict_check_node(seed_node, "spread_activation(seed_node)")?;
        self.activation.clear();
        self.activation.insert(seed_node, seed_strength);
        self.temporal_window.push_back((seed_node, seed_strength, self.tick));
        if self.temporal_window.len() > TEMPORAL_WINDOW_SIZE {
            self.temporal_window.pop_front();
        }

        let mut queue: VecDeque<(u64, f32, u8)> = VecDeque::new();
        queue.push_back((seed_node, seed_strength, 0));

        while let Some((node, strength, depth)) = queue.pop_front() {
            if depth >= MAX_SPREAD_DEPTH {
                continue;
            }

            let connections = self.get_connections_internal(node);
            for (receiver, weight) in connections {
                let incoming = strength * weight;
                let threshold = self
                    .node_index
                    .get(&receiver)
                    .map_or(DEFAULT_THRESHOLD, |m| m.threshold);
                if incoming < threshold {
                    continue;
                }

                let current = self.activation.get(&receiver).copied().unwrap_or(0.0);
                if incoming > current {
                    self.activation.insert(receiver, incoming);
                    self.temporal_window.push_back((receiver, incoming, self.tick));
                    if self.temporal_window.len() > TEMPORAL_WINDOW_SIZE {
                        self.temporal_window.pop_front();
                    }
                    queue.push_back((receiver, incoming, depth.saturating_add(1)));
                }
            }
        }

        self.tick = self.tick.saturating_add(1);
        Ok(())
    }

    fn get_active_nodes(&self) -> Vec<(u64, f32)> {
        let mut out: Vec<(u64, f32)> = self.activation.iter().map(|(k, v)| (*k, *v)).collect();
        out.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        out
    }

    fn compute_cd(&mut self, stimulus: u64, context: Vec<u64>) -> PyResult<Vec<(u64, f64)>> {
        self.strict_check_node(stimulus, "compute_cd(stimulus)")?;
        for ctx in &context {
            self.strict_check_node(*ctx, "compute_cd(context)")?;
        }

        let actions = self.get_connections_internal(stimulus);
        if actions.is_empty() {
            return Ok(Vec::new());
        }

        let mut out: Vec<(u64, f64)> = Vec::new();
        for (action_id, value) in &actions {
            let cost_conns = self.get_connections_internal(*action_id);
            let cost = if cost_conns.is_empty() {
                1.0_f64
            } else {
                let total: f32 = cost_conns.iter().map(|(_, w)| *w).sum();
                (total / cost_conns.len() as f32) as f64
            };

            let mut opp_weights: Vec<f64> = Vec::new();
            for ctx in &context {
                for (target, w) in self.get_connections_internal(*ctx) {
                    if target == *action_id {
                        opp_weights.push(w as f64);
                    }
                }
            }

            let opportunity = if opp_weights.is_empty() {
                0.5
            } else {
                opp_weights.iter().sum::<f64>() / opp_weights.len() as f64
            };

            let cd = if cost == 0.0 {
                f64::MAX
            } else {
                (*value as f64 * opportunity) / cost
            };
            out.push((*action_id, cd));
        }

        out.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        Ok(out)
    }
    fn form_synapses_from_window(&mut self) -> u32 {
        let nodes: Vec<(u64, f32)> = self
            .temporal_window
            .iter()
            .map(|(node_id, strength, _)| (*node_id, *strength))
            .collect();

        let mut formed = 0_u32;
        for i in 0..nodes.len() {
            let (sender, s_strength) = nodes[i];
            if !self.node_index.contains_key(&sender) {
                continue;
            }

            let sender_thr = self
                .node_index
                .get(&sender)
                .map_or(DEFAULT_THRESHOLD, |m| m.threshold);
            if s_strength < sender_thr {
                continue;
            }
            if self.synapse_count_for(sender) >= MAX_SYNAPSES_PER_NODE {
                continue;
            }

            for j in 0..nodes.len() {
                if i == j {
                    continue;
                }

                let (receiver, r_strength) = nodes[j];
                if !self.node_index.contains_key(&receiver) {
                    continue;
                }

                let prob = s_strength * r_strength;
                if rand_f32() > prob {
                    continue;
                }

                let in_delta = self
                    .delta_index
                    .get(&sender)
                    .map_or(false, |m| m.contains_key(&receiver));
                if in_delta {
                    continue;
                }

                let base_syn = self.get_cached_or_load_base(sender);
                let in_base = base_syn.iter().any(|s| s.receiver_id == receiver);
                if in_base {
                    continue;
                }

                let ts = self.tick;
                let entry = DeltaEntry {
                    sender_id: sender,
                    receiver_id: receiver,
                    weight: INITIAL_WEIGHT,
                    timestamp: ts,
                };
                self.append_delta_entry(&entry);
                self.delta_index
                    .entry(sender)
                    .or_default()
                    .insert(receiver, (INITIAL_WEIGHT, ts));
                self.invalidate_sender_cache(sender);
                formed = formed.saturating_add(1);
            }
        }

        formed
    }

    fn update_weight(&mut self, sender: u64, receiver: u64, new_weight: f32) -> PyResult<()> {
        self.strict_check_node(sender, "update_weight(sender)")?;
        self.strict_check_node(receiver, "update_weight(receiver)")?;

        let weight = new_weight.max(0.0).min(1.0);
        if let Some(runtime) = self.async_runtime.as_ref() {
            let owner = self.owner_shard(sender);
            let (tx, rx) = oneshot::channel();
            let cmd = ShardCommand::UpdateEdge {
                sender,
                receiver,
                weight,
                reply: tx,
            };
            if runtime.shard_txs[owner].send(cmd).is_err() {
                return Err(PyValueError::new_err("failed to route update_weight to owner shard"));
            }
            let ok = runtime.rt.block_on(async { rx.await.unwrap_or(false) });
            if !ok {
                return Err(PyValueError::new_err("async shard rejected update_weight"));
            }
        }

        let ts = self.tick;
        self.tick = self.tick.saturating_add(1);

        self.delta_index
            .entry(sender)
            .or_default()
            .insert(receiver, (weight, ts));

        let entry = DeltaEntry {
            sender_id: sender,
            receiver_id: receiver,
            weight,
            timestamp: ts,
        };
        self.append_delta_entry(&entry);
        self.invalidate_sender_cache(sender);
        Ok(())
    }

    fn consolidate(&mut self) -> (u32, u32) {
        let async_exists = self.async_runtime.is_some();
        if async_exists {
            if let Some(runtime) = self.async_runtime.as_ref() {
                runtime.rt.block_on(async {
                    let mut s = runtime.shared.lock().await;
                    s.ingress_paused = true;
                });
                for tx in &runtime.shard_txs {
                    let (ack_tx, ack_rx) = oneshot::channel();
                    let _ = tx.send(ShardCommand::Flush { reply: ack_tx });
                    let _ = runtime.rt.block_on(async { ack_rx.await });
                }
                self.sync_async_state_from_shared();
            }
        }

        let mut merged = 0_u32;
        let mut pruned = 0_u32;

        let senders: Vec<u64> = self.delta_index.keys().copied().collect();
        for sender in &senders {
            let mut synapses = self.load_from_base(*sender);
            if let Some(delta) = self.delta_index.get(sender) {
                for (receiver, (weight, _)) in delta {
                    if let Some(existing) = synapses.iter_mut().find(|s| s.receiver_id == *receiver) {
                        existing.weight = *weight;
                    } else {
                        synapses.push(Synapse {
                            receiver_id: *receiver,
                            weight: *weight,
                        });
                    }
                    merged = merged.saturating_add(1);
                }
            }

            if !synapses.is_empty() {
                let avg = synapses.iter().map(|s| s.weight).sum::<f32>() / synapses.len() as f32;
                let threshold = avg * PRUNE_RATIO;
                let before = synapses.len();
                synapses.retain(|s| s.weight >= threshold);
                synapses.sort_by(|a, b| b.weight.partial_cmp(&a.weight).unwrap_or(std::cmp::Ordering::Equal));
                pruned = pruned.saturating_add((before - synapses.len()) as u32);
            }

            if let Some(meta) = self.node_index.get_mut(sender) {
                meta.synapse_count = synapses.len() as u32;
            }
        }

        self.rebuild_base_bin();
        self.delta_index.clear();
        self.reset_delta_file();
        self.temporal_window.clear();
        self.activation.clear();

        // Keep only refreshed pinned hotset after major merge/prune.
        self.base_cache.clear();
        self.pinned_cache.clear();
        self.refresh_cache_budget();
        self.recompute_pinned_set(true);

        if async_exists {
            let (adjacency, threshold) = self.build_async_snapshot();
            if let Some(runtime) = self.async_runtime.as_ref() {
                runtime.rt.block_on(async {
                    let mut s = runtime.shared.lock().await;
                    s.adjacency = adjacency;
                    s.threshold = threshold;
                    s.activation.clear();
                    s.global_queue_len = 0;
                    s.per_shard_queue_len = vec![0; s.shard_count];
                    s.ingress_paused = false;
                });
                self.sync_async_state_from_shared();
            }
        }

        println!("[Konsolidasi] merged={} pruned={}", merged, pruned);
        (merged, pruned)
    }

    fn status(&self) -> String {
        let delta_total: usize = self.delta_index.values().map(|m| m.len()).sum();
        let budget_mb = self.cache_budget_bytes as f64 / (1024.0 * 1024.0);
        let cache_mb = self.cache_bytes_est as f64 / (1024.0 * 1024.0);
        let chunk_count = self.chunk_file_starts().len();
        let mut active_count = self.activation.len();
        let mut queue_len = self.async_state.global_queue_len;
        let mut guard_mode = self.async_state.guard_mode.clone();
        if let Some(runtime) = self.async_runtime.as_ref() {
            let snap = runtime.rt.block_on(async {
                let s = runtime.shared.lock().await;
                (s.activation.len(), s.global_queue_len, s.guard_mode.clone())
            });
            active_count = snap.0;
            queue_len = snap.1;
            guard_mode = snap.2;
        }

        format!(
            "Nodes={} | Chunks={} | Delta nodes={} entries={} | Active={} | Tick={} | reg_ver={} | pinned_nodes={} | lru_nodes={} | cache_budget_mb={:.1} | cache_bytes_est_mb={:.1} | async_on={} | shards={} | global_queue_len={} | guard_mode={}",
            self.node_index.len(),
            chunk_count,
            self.delta_index.len(),
            delta_total,
            active_count,
            self.tick,
            self.registry_version,
            self.pinned_cache.len(),
            self.base_cache.len(),
            budget_mb,
            cache_mb,
            self.async_state.enabled,
            self.async_state.shard_count,
            queue_len,
            guard_mode
        )
    }

    fn get_activation(&self) -> Vec<(u64, f32)> {
        if let Some(runtime) = self.async_runtime.as_ref() {
            let mut out: Vec<(u64, f32)> = runtime.rt.block_on(async {
                let s = runtime.shared.lock().await;
                s.activation.iter().map(|(k, v)| (*k, *v)).collect()
            });
            out.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            return out;
        }
        let mut out: Vec<(u64, f32)> = self.activation.iter().map(|(k, v)| (*k, *v)).collect();
        out.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        out
    }
}

async fn shard_actor_loop(
    shard_id: usize,
    mut rx: mpsc::UnboundedReceiver<ShardCommand>,
    shard_txs: Vec<mpsc::UnboundedSender<ShardCommand>>,
    shared: Arc<TokioMutex<AsyncShared>>,
) {
    while let Some(cmd) = rx.recv().await {
        match cmd {
            ShardCommand::Stimulus {
                node_id,
                strength,
                source,
                origin_tick,
                reply,
            } => {
                decrement_queue_on_pop(shard_id, &shared).await;
                process_seed_message(
                    shard_id,
                    node_id,
                    strength,
                    origin_tick,
                    Some(source),
                    &shard_txs,
                    &shared,
                )
                .await;
                let _ = reply.send(true);
            }
            ShardCommand::Hop {
                node_id,
                strength,
                origin_tick,
                source_shard: _,
            } => {
                decrement_queue_on_pop(shard_id, &shared).await;
                process_seed_message(
                    shard_id,
                    node_id,
                    strength,
                    origin_tick,
                    None,
                    &shard_txs,
                    &shared,
                )
                .await;
            }
            ShardCommand::UpdateEdge {
                sender,
                receiver,
                weight,
                reply,
            } => {
                decrement_queue_on_pop(shard_id, &shared).await;
                let mut s = shared.lock().await;
                let list = s.adjacency.entry(sender).or_default();
                if let Some(existing) = list.iter_mut().find(|e| e.receiver_id == receiver) {
                    existing.weight = weight;
                } else {
                    list.push(AsyncSynapse { receiver_id: receiver, weight });
                }
                let _ = reply.send(true);
            }
            ShardCommand::Flush { reply } => {
                let _ = reply.send(());
            }
            ShardCommand::Stop => {
                break;
            }
        }
    }
}

async fn decrement_queue_on_pop(shard_id: usize, shared: &Arc<TokioMutex<AsyncShared>>) {
    let mut s = shared.lock().await;
    s.global_queue_len = s.global_queue_len.saturating_sub(1);
    if let Some(slot) = s.per_shard_queue_len.get_mut(shard_id) {
        *slot = slot.saturating_sub(1);
    }
}

async fn process_seed_message(
    shard_id: usize,
    node_id: u64,
    strength: f32,
    _origin_tick: u64,
    _source: Option<String>,
    shard_txs: &[mpsc::UnboundedSender<ShardCommand>],
    shared: &Arc<TokioMutex<AsyncShared>>,
) {
    let mut queue: VecDeque<(u64, f32, u8)> = VecDeque::new();
    queue.push_back((node_id, strength.max(0.0).min(1.0), 0));

    while let Some((node, node_strength, depth)) = queue.pop_front() {
        if depth >= MAX_SPREAD_DEPTH {
            continue;
        }

        let (connections, threshold_map, shard_count) = {
            let s = shared.lock().await;
            (
                s.adjacency.get(&node).cloned().unwrap_or_default(),
                s.threshold.clone(),
                s.shard_count,
            )
        };

        for syn in connections {
            let incoming = node_strength * syn.weight;
            let threshold = threshold_map
                .get(&syn.receiver_id)
                .copied()
                .unwrap_or(DEFAULT_THRESHOLD);
            if incoming < threshold {
                continue;
            }

            {
                let mut s = shared.lock().await;
                let slot = s.activation.entry(syn.receiver_id).or_insert(0.0);
                if incoming > *slot {
                    *slot = incoming;
                } else {
                    continue;
                }
            }

            let target_shard = if shard_count == 0 {
                0
            } else {
                (syn.receiver_id as usize) % shard_count
            };
            if target_shard == shard_id {
                queue.push_back((syn.receiver_id, incoming, depth.saturating_add(1)));
            } else {
                {
                    let mut s = shared.lock().await;
                    s.hop_total = s.hop_total.saturating_add(1);
                    s.global_queue_len = s.global_queue_len.saturating_add(1);
                    if let Some(slot) = s.per_shard_queue_len.get_mut(target_shard) {
                        *slot = slot.saturating_add(1);
                    }
                }
                let _ = shard_txs[target_shard].send(ShardCommand::Hop {
                    node_id: syn.receiver_id,
                    strength: incoming,
                    origin_tick: 0,
                    source_shard: shard_id,
                });
            }
        }
    }

    let now_ms = RagpEngine::now_ms();
    let mut s = shared.lock().await;
    s.processed_total = s.processed_total.saturating_add(1);
    if let Some(slot) = s.per_shard_processed.get_mut(shard_id) {
        *slot = slot.saturating_add(1);
    }
    if s.last_rate_ts_ms == 0 {
        s.last_rate_ts_ms = now_ms;
        s.last_rate_processed_total = s.processed_total;
        s.processed_per_sec = 0.0;
    } else {
        let dt_ms = now_ms.saturating_sub(s.last_rate_ts_ms);
        if dt_ms >= 200 {
            let dp = s.processed_total.saturating_sub(s.last_rate_processed_total);
            s.processed_per_sec = (dp as f64) / (dt_ms as f64 / 1000.0);
            s.last_rate_ts_ms = now_ms;
            s.last_rate_processed_total = s.processed_total;
        }
    }
}

fn rand_f32() -> f32 {
    use std::time::{SystemTime, UNIX_EPOCH};

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    let mixed = nanos.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    (mixed as f32) / (u32::MAX as f32)
}

#[pymodule]
fn ctn_engine(_py: Python, m: &PyModule) -> PyResult<()> {
    m.add_class::<RagpEngine>()?;
    Ok(())
}
