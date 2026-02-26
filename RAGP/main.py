from __future__ import annotations

import os
import shutil
import sys

from environment import translate
from ragp_bootstrap import innate_registry_version, node_pool_full, seed_initial_knowledge
from ragp_loop import run_survival_loop

try:
    from ctn_engine import RagpEngine
except ImportError:
    print("ERROR: Modul 'ctn_engine' belum terpasang untuk interpreter ini.")
    print("Gunakan interpreter .venv dan jalankan: maturin develop --release")
    sys.exit(1)


STORAGE_DIR = os.path.join(os.getcwd(), "ragp_storage")

DEFAULT_MAX_STEPS = 100
DEFAULT_SEED = 42

# Shared RAM buffer for short-term reward traces.
hippocampus: dict = {}


def _truthy(value: str | None) -> bool:
    if value is None:
        return False
    return value.strip().lower() in {"1", "true", "yes", "on"}


def should_reset(argv: list[str]) -> bool:
    return ("--reset" in argv) or _truthy(os.getenv("RAGP_RESET_STORAGE"))


def base_file_exists(storage_dir: str) -> bool:
    base_path = os.path.join(storage_dir, "base.bin")
    return os.path.exists(base_path) and os.path.getsize(base_path) >= 14


def rescorla_wagner(old_weight: float, reward: float, count: int = 1) -> float:
    alpha = 1.0 / max(count, 1)
    new_w = old_weight + alpha * (reward - old_weight)
    return max(0.0, min(1.0, new_w))


def consolidate_hippocampus(engine: RagpEngine, buffer: dict, verbose: bool = True):
    if not buffer:
        if verbose:
            print("[Konsolidasi] Buffer kosong.")
        return

    mean_signal = sum(abs(e["peak"]) for e in buffer.values()) / len(buffer)
    if verbose:
        print(f"[Konsolidasi] {len(buffer)} entri | mean_signal={mean_signal:.3f}")

    consolidated = 0
    for (sender, receiver), entry in buffer.items():
        if abs(entry["peak"]) <= mean_signal:
            continue

        try:
            old_weight = dict(engine.get_connections(sender)).get(receiver, 0.5)
        except ValueError as exc:
            if verbose:
                print(f"  [Skip] {exc}")
            continue
        new_weight = rescorla_wagner(old_weight, entry["acc"], entry["count"])
        engine.update_weight(sender, receiver, new_weight)
        consolidated += 1

        if verbose:
            print(
                f"  [{translate(sender)}->{translate(receiver)}] "
                f"{old_weight:.3f} -> {new_weight:.3f} (peak={entry['peak']:+.2f})"
            )

    if verbose:
        print(f"[Konsolidasi] {consolidated}/{len(buffer)} entri ditulis.")


def build_konsolidasi_fn():
    def _fn(hpc: dict, engine: RagpEngine, verbose: bool = True):
        consolidate_hippocampus(engine, hpc, verbose=verbose)
        merged, pruned = engine.consolidate()
        if verbose:
            print(f"[Engine] merged={merged} pruned={pruned}")
            print(f"[Status] {engine.status()}")

    return _fn


def main():
    os.environ.setdefault("RAGP_INNATE_REGISTRY_VERSION", str(innate_registry_version()))
    async_enabled = _truthy(os.getenv("RAGP_ASYNC"))
    reset_requested = should_reset(sys.argv[1:])
    if reset_requested and os.path.exists(STORAGE_DIR):
        shutil.rmtree(STORAGE_DIR)
        print(f"[Init] Storage lama dihapus: {STORAGE_DIR}")

    first_init = not base_file_exists(STORAGE_DIR)
    engine = RagpEngine(STORAGE_DIR)
    migration_status = engine.ensure_innate_registry(node_pool_full())
    print(f"[Registry] {migration_status}")
    if async_enabled:
        msg = engine.start_async_runtime(None)
        print(f"[Async] {msg}")

    if first_init:
        print(f"[Init] Node pool dibuat. {engine.status()}")
        seed_initial_knowledge(engine)
    else:
        print(f"[Resume] Melanjutkan state base+delta. {engine.status()}")

    run_survival_loop(
        engine=engine,
        hippocampus=hippocampus,
        konsolidasi_fn=build_konsolidasi_fn(),
        max_steps=DEFAULT_MAX_STEPS,
        seed=DEFAULT_SEED,
        verbose=True,
    )


if __name__ == "__main__":
    main()
