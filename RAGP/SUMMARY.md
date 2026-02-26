# RAGP5 - PROJECT SUMMARY
## Always append new information to this summary.md after any changes in the project with timestamp.


## Current State
**Updated:** 2026-02-26:14:50:00


Phase 10 baseline plus gap-closure is implemented:
- Binary engine active (`base.bin + delta.bin`)
- Runtime loop uses `ragp_loop.py`
- Default startup resumes previous state
- Hybrid memory cache (`Pinned + LRU`) with adaptive RAM budget

---

## Storage Model

```text
ragp_storage/
  base.bin   -> manifest index (header + node records)
  base_000001_000100.bin, base_000101_000200.bin, ... -> synapse chunks by sender range
  delta.bin  -> latest updates since last consolidate (append-only)
```

Startup behavior:
1. Load `base.bin` node index.
2. If legacy monolithic base is detected, migrate automatically to chunk files.
3. Validate/auto-migrate innate registry via `ensure_innate_registry(node_ids)`.
4. Ensure `delta.bin` exists.
5. Replay `delta.bin` entries with CRC32 validation into `delta_index`.
6. Continue from latest timestamp (`tick`).

`delta.bin` is reset after every `consolidate()`.

---

## Memory Model (RAM)

```text
node_index      : HashMap<u64, NodeMeta>
delta_index     : HashMap<u64, HashMap<u64,(f32,u32)>>
activation      : HashMap<u64, f32>
temporal_window : VecDeque<(u64, f32, u32)>

pinned_cache    : HashMap<u64, Vec<Synapse>>
base_cache      : LruCache<u64, Vec<Synapse>>
pinned_set      : HashSet<u64>
access_count    : HashMap<u64, u32>
```

Cache policy:
- Source of truth remains disk (`base.bin + delta.bin`).
- `pinned_cache` keeps hot/important nodes resident.
- `base_cache` handles recency via LRU.
- Budget is adaptive to available RAM and constrained by min/max caps.

---

## Runtime Behavior

Main path:
1. `main.py` parses startup mode.
2. `main.py` always calls `ensure_innate_registry()` to keep innate node set in sync.
3. On first init: seed basic instinct links.
4. On resume: continue existing storage.
5. `run_survival_loop()` in `ragp_loop.py` drives perception-action-learning cycle.

Loop highlights:
- Sensor IDs converted to `int` for Rust API.
- `spread_activation()` called per active sensor.
- Action chosen from `compute_cd()`.
- Environment action still string-based (`env.apply_action(str(action_id))`).
- Hebbian formation via `form_synapses_from_window()`.
- Consolidation every 20 steps.

---

## Public Runtime Controls

Reset control:
- CLI: `--reset`
- Env: `RAGP_RESET_STORAGE=1`

Cache controls:
- `RAGP_CACHE_POLICY` (default `pinned_lru`)
- `RAGP_CACHE_RAM_FRACTION` (default `0.25`)
- `RAGP_CACHE_RAM_MIN_MB` (default `256`)
- `RAGP_CACHE_RAM_MAX_MB` (default `1536`)
- `RAGP_CACHE_PIN_FRACTION` (default `0.35`)

Innate registry controls:
- `RAGP_INNATE_REGISTRY_VERSION` (default `1`)
- Unknown node policy: hard reject (`ValueError`) for `get_connections`, `spread_activation`, `compute_cd`, `update_weight`.
- Registry change policy: auto migrate (`base+delta` preserved where node IDs still valid, invalid links pruned).

---

## Engine Status Metrics

`status()` now includes:
- `pinned_nodes`
- `lru_nodes`
- `cache_budget_mb`
- `cache_bytes_est_mb`

This is used to monitor whether cache stays performant but proportional to other RAM consumers.

---

## Notes

- `base_cache` is performance-only cache, not memory authority.
- `base.bin` and `delta.bin` remain authoritative.
- Consolidation keeps `delta` small for fast startup and stable replay cost.

---

## MCP Integration Update (ragp-local)
**Updated:** 2026-02-26:15:24:26

Global MCP server `ragp-local` is installed and registered in Codex global config.

Runtime MCP path:
- `C:\Users\SYUAIB\.codex\mcp_servers\ragp_mcp\mcp_server.py`

Registered config section:
- `[mcp_servers.ragp-local]`
- Python executable: `D:\code\RAGP5\RAGP\.venv\Scripts\python.exe`

Available tools:
- `ragp_status`
- `ragp_activation_top`
- `ragp_predict`
- `ragp_spread`
- `ragp_get_connections`
- `ragp_update_weight`
- `ragp_consolidate`
- `ragp_registry_sync`

Tool functions:
- `ragp_status`: return ringkasan status engine (nodes/chunks/delta/active/tick/cache/registry).
- `ragp_activation_top`: return node aktivasi tertinggi beserta strength.
- `ragp_predict`: hitung kandidat aksi dari stimulus + context (score `cd`).
- `ragp_spread`: kirim stimulus node ke engine untuk menyebarkan aktivasi.
- `ragp_get_connections`: baca koneksi keluar node (`base + delta` overlay).
- `ragp_update_weight`: ubah bobot koneksi sender->receiver (write).
- `ragp_consolidate`: gabungkan delta ke base dan pruning koneksi lemah (write).
- `ragp_registry_sync`: sinkronkan daftar node innate + auto-migrate storage (write).

Current operational constraint (user policy):
- Use read-only + stimulus-only interaction.
- Allowed: `ragp_status`, `ragp_activation_top`, `ragp_predict`, `ragp_get_connections`, `ragp_spread`.
- Disallowed for routine operation: write/mutation tools (`ragp_update_weight`, `ragp_consolidate`, `ragp_registry_sync`) unless explicitly requested.

Smoke-check status:
- MCP server is reachable after restart.
- `ragp_status` and `ragp_predict` return valid outputs.
- `ragp_spread` successfully triggers activation propagation.

---

## Refactor Update (Bootstrap Module)
**Updated:** 2026-02-26:15:49:55

Main bootstrap hardcode has been moved out of `main.py` into standalone module `ragp_bootstrap.py`.

Changes:
- `node_pool_full()` now provided by `ragp_bootstrap.py` (reads `RAGP_NODE_MAX`, default `109`).
- `innate_registry_version()` now provided by `ragp_bootstrap.py`.
- `seed_initial_knowledge(engine)` moved to `ragp_bootstrap.py`.
- `main.py` now acts as runtime orchestrator and imports bootstrap functions.

Validation:
- `python -m py_compile main.py ragp_bootstrap.py` passed.
- `import main` from venv passed.

---

## Refactor Update (Bootstrap Data-Driven, Stage 2)
**Updated:** 2026-02-26:15:52:00

Bootstrap seed map is now externalized into data file:
- Added `ragp_bootstrap_config.json` as source for:
  - `registry_version`
  - `node_max`
  - `seed_links` (sender, receiver, weight)

Module behavior update:
- `ragp_bootstrap.py` now loads config from:
  - `RAGP_BOOTSTRAP_CONFIG` env, or
  - default file `ragp_bootstrap_config.json` in project root.
- `innate_registry_version()` reads env override first, then config value.
- `node_pool_full()` reads `RAGP_NODE_MAX` override first, then config `node_max`.
- `seed_initial_knowledge()` uses `seed_links()` from config instead of inline hardcode list.

Safety/fallback:
- If config file missing/invalid, module falls back to default in-module values.
- Seed link values are sanitized (`sender/receiver > 0`, `weight` clamped to `[0,1]`).

Validation:
- `python -m py_compile ragp_bootstrap.py main.py` passed.
- Runtime read check: registry=1, node_pool=109, seed_links=16 loaded from config.

---

## Primitive Stimulus Mapping Update
**Updated:** 2026-02-26:15:58:39

Primitive human-inspired stimulus bundle has been added to bootstrap config with minimal initial weights.

Config changes:
- `ragp_bootstrap_config.json`:
  - `node_max` increased from `109` to `140`.
  - Added low-weight primitive seed links (`0.01` to `0.03`).
  - Added `node_semantics` dictionary for new IDs.

New primitive sensor nodes:
- `110`: `SENSOR_CO2_DISTRESS`
- `111`: `SENSOR_HAUS`
- `112`: `SENSOR_STRES_TERMAL`
- `113`: `SENSOR_STARTLE`
- `114`: `SENSOR_O2_RENDAH`

New context nodes:
- `120`: `KONTEKS_SUARA_KERAS`
- `121`: `KONTEKS_PANAS_EKSTREM`
- `122`: `KONTEKS_DINGIN_EKSTREM`
- `123`: `KONTEKS_MULUT_KERING`
- `124`: `KONTEKS_NAPAS_BERAT`

New action nodes:
- `130`: `AKSI_CARI_UDARA`
- `131`: `AKSI_CARI_MINUM`
- `132`: `AKSI_MENDINGIN`
- `133`: `AKSI_MENGHANGAT`
- `134`: `AKSI_FREEZE_ORIENT`
- `135`: `AKSI_MINTA_BANTUAN`

Compatibility note:
- `environment.py` semantic map now includes these new IDs for `translate()`.
- Main survival loop action filter (`ragp_loop.py`) is unchanged, so new actions are currently prepared in graph but not yet selected by the default loop.

Validation:
- JSON config parse passed.
- Bootstrap read check: `node_pool=140`, `seed_links=27`.
- `python -m py_compile ragp_bootstrap.py environment.py main.py` passed.

---

## Primitive Stimulus Mapping Update (Complete Pack v1)
**Updated:** 2026-02-26:16:04:14

Expanded primitive bundle to a fuller interoceptive set with minimal initial weights.

Updates:
- `ragp_bootstrap_config.json` now sets:
  - `node_max=220`
  - `seed_links=64`
  - `node_semantics=65`
- Added primitive sensors beyond initial 5:
  - pain, nausea, bladder fullness, itch, sleep pressure,
  - urgent hunger, palpitations, fever/inflammation,
  - vestibular disorientation, postural instability.
- Added richer context and action nodes for primitive regulation behaviors.

Hardcode reduction:
- `environment.py` now auto-loads `node_semantics` from bootstrap config file
  (via `RAGP_BOOTSTRAP_CONFIG` or default project config path).
- This removes repeated manual semantic hardcode for new node IDs.

Technical note:
- JSON loaders in `ragp_bootstrap.py` and `environment.py` now use `utf-8-sig`
  to handle BOM-safe parsing.

Validation:
- Runtime read check: `node_pool=220`, `seed_links=64`, `reg=1`.
- `translate(115)`, `translate(170)`, `translate(189)` resolve to semantic labels.
- `python -m py_compile ragp_bootstrap.py environment.py main.py` passed.

---

## MCP Update (ragp-audio-local v0)
**Updated:** 2026-02-26:16:17:14

Added a new global MCP server for microphone pipeline:
- `C:\Users\SYUAIB\.codex\mcp_servers\ragp_audio_mcp\mcp_server.py`

Registered in global config:
- `[mcp_servers.ragp-audio-local]`
- Python: `D:\code\RAGP5\RAGP\.venv\Scripts\python.exe`

Tools (read/stimulus-only):
- `audio_status`: check mic backend and defaults.
- `audio_listen_features`: capture mic and extract features (`rms`, `peak`, `zcr`, `spectral_centroid_hz`, `delta_rms`).
- `audio_to_nodes`: map features to node candidates (`120`, `140`, `113`, `155`).
- `audio_capture_to_nodes`: capture then map directly.
- `audio_stimulate_ragp`: capture->map->`spread_activation` into RAGP, then return activation + prediction.

Dependencies:
- Installed in RAGP venv: `numpy`, `sounddevice` (plus existing `mcp`).

Validation:
- Module import passed.
- `audio_status` returns active audio backend and default device.
- `audio_to_nodes` smoke checks passed for loud/sudden and quiet synthetic inputs.

---

## Runtime Update (Autonomous Audio Stimulus Loop)
**Updated:** 2026-02-26:16:24:59

Added standalone runtime script so RAGP can listen and stimulate its node network without manual MCP calls.

New file:
- `ragp_audio_autonomy.py`

Flow:
- microphone capture -> feature extraction -> audio->node mapping -> `spread_activation` -> optional `compute_cd` preview
- design is stimulus-only (no `update_weight`, no `consolidate`)

Default behavior:
- storage path now defaults to project-local `RAGP/ragp_storage` (script directory based).
- supports `--no-mic` for dry run.

Example run:
```powershell
D:\code\RAGP5\RAGP\.venv\Scripts\python.exe D:\code\RAGP5\RAGP\ragp_audio_autonomy.py --storage-dir D:\code\RAGP5\RAGP\ragp_storage
```

Validation:
- `python -m py_compile ragp_audio_autonomy.py` passed.
- dry-run loop with `--no-mic --max-loops 1` passed and triggered node stimulus output.

---

## Runtime Update (Sharded Async Ingress v1)
**Updated:** 2026-02-26:16:56:44

Implemented async/sharded ingress surface in Rust core with sender-based ownership and runtime controls.

Rust core changes (`src/lib.rs`):
- Added async runtime APIs:
  - `start_async_runtime(config=None)`
  - `stop_async_runtime()`
  - `submit_stimulus(node_id, strength, source='unknown', ts_ms=None)`
  - `submit_stimuli(batch)`
  - `get_async_metrics()`
  - `set_async_policy(...)`
- Ownership rule:
  - `owner_shard = sender_id % shard_count`
- Added guard/coalesce/runtime metrics:
  - `global_queue_len`
  - `per_shard_queue_len`
  - `per_shard_processed`
  - `dropped_total`
  - `coalesced_total`
  - `hop_total`
  - `processed_total`
  - `processed_per_sec`
  - `guard_mode`
- `status()` now includes async state:
  - `async_on`, `shards`, `global_queue_len`, `guard_mode`
- Consolidate behavior:
  - ingress pause flag (`ingress_paused`) enabled during consolidate as barrier guard.

Python/runtime integration:
- `main.py`:
  - optional async boot path via `RAGP_ASYNC=1`.
- `ragp_loop.py`:
  - uses `submit_stimuli` when async runtime is active; fallback to `spread_activation` when async off.
- `ragp_audio_autonomy.py`:
  - async-first stimulation path (`submit_stimuli`) with `--force-sync` fallback.

MCP updates:
- `ragp-local` MCP:
  - added async tools:
    - `ragp_async_start`
    - `ragp_async_stop`
    - `ragp_async_metrics`
    - `ragp_async_set_policy`
    - `ragp_submit_stimulus`
    - `ragp_submit_stimuli`
- `ragp-audio-local` MCP:
  - `audio_stimulate_ragp` now routes to `submit_stimuli` when async is ON,
    fallback to `spread_activation` when async is OFF.

Validation:
- `cargo check` passed (with `PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1`).
- `maturin develop --release` passed.
- `py_compile` passed:
  - `main.py`
  - `ragp_loop.py`
  - `ragp_audio_autonomy.py`
  - global MCP servers (`ragp_mcp`, `ragp_audio_mcp`)
- Smoke test (Python binding):
  - start async (4 shards) OK
  - batch submit OK (`accepted=2`, `coalesced=1`)
  - async metrics returned valid shard/global counters
  - stop async OK

---

## Runtime Update (Full Tokio Actor Runtime v2)
**Updated:** 2026-02-26:17:08:19

Upgraded async core from scaffold mode into Tokio actor runtime with sharded inbox workers.

Core architecture (`src/lib.rs`):
- Tokio runtime enabled in Rust crate (`tokio` dependency).
- Added async runtime container:
  - `AsyncActorRuntime { rt, shard_txs, shared, global_tick }`
- Added per-shard actor loop:
  - `shard_actor_loop(...)`
  - single-consumer unbounded inbox per shard
- Added message protocol:
  - `Stimulus`
  - `Hop` (cross-shard propagation)
  - `UpdateEdge` (sender-owner serialized write path)
  - `Flush` (barrier sync)
  - `Stop` (graceful shutdown)
- Ownership remains sender-centric:
  - `owner_shard = sender_id % shard_count`

Ingress/runtime behavior:
- `start_async_runtime` now:
  - builds adjacency+threshold snapshot from current `base+delta`,
  - spins Tokio runtime,
  - spawns N shard actors.
- `submit_stimulus` now:
  - enqueues to owner shard,
  - applies guard mode at ingress,
  - waits ack via oneshot.
- `submit_stimuli`:
  - coalesces `(node_id, source)` duplicates before routing.
- `update_weight` (async ON):
  - routed first to owner shard (`UpdateEdge`) for shard-serialized graph mutation,
  - then persisted to engine delta log/source-of-truth as before.
- `consolidate` (async ON):
  - sets ingress paused,
  - sends `Flush` barrier to all shards,
  - runs merge/prune,
  - rebuilds async adjacency snapshot,
  - resumes ingress.

Metrics/status:
- Async metrics are now sourced from shared Tokio state:
  - queue len global/per shard
  - processed total/per shard
  - coalesced/dropped/hop
  - processed/sec
  - guard mode
- `status()` reads async queue/guard/active state from shared runtime when active.
- `get_activation()` reads async activation map when runtime is on.

Validation:
- `cargo check` passed (with `PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1`).
- `maturin develop --release` passed.
- Smoke test passed:
  - start async 4 shards
  - submit batch (with coalesce)
  - read async metrics
  - update weight while async active
  - consolidate with barrier
  - stop async
