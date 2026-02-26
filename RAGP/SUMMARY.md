# RAGP5 - PROJECT SUMMARY
**Updated:** 2026-02-26

---

## Current State

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
4. `run_survival_loop()` in `ragp_loop.py` drives perception-action-learning cycle.

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
