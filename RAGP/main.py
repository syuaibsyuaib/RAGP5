from __future__ import annotations

import os
import shutil
import sys

from environment import translate
from ragp_loop import run_survival_loop

try:
    from ctn_engine import RagpEngine
except ImportError:
    print("ERROR: Modul 'ctn_engine' belum terpasang untuk interpreter ini.")
    print("Gunakan interpreter .venv dan jalankan: maturin develop --release")
    sys.exit(1)


STORAGE_DIR = os.path.join(os.getcwd(), "ragp_storage")
NODE_POOL_FULL = list(range(1, 110))
INNATE_REGISTRY_VERSION = 1

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


def seed_initial_knowledge(engine: RagpEngine):
    print("[Init] Menanamkan insting dasar...")

    engine.update_weight(1, 45, 0.3)    # BAHAYA -> LARI
    engine.update_weight(1, 88, 0.2)    # BAHAYA -> SEMBUNYI
    engine.update_weight(1, 12, 0.1)    # BAHAYA -> DIAM

    engine.update_weight(103, 106, 0.3) # LAPAR -> CARI_MAKAN
    engine.update_weight(103, 107, 0.2) # LAPAR -> MAKAN

    engine.update_weight(100, 108, 0.3) # LELAH -> ISTIRAHAT
    engine.update_weight(100, 109, 0.2) # LELAH -> TIDUR

    engine.update_weight(104, 109, 0.3) # SAKIT -> TIDUR
    engine.update_weight(104, 108, 0.2) # SAKIT -> ISTIRAHAT

    engine.update_weight(45, 100, 0.15) # Cost LARI
    engine.update_weight(88, 100, 0.05) # Cost SEMBUNYI
    engine.update_weight(12, 100, 0.02) # Cost DIAM
    engine.update_weight(106, 100, 0.10) # Cost CARI_MAKAN
    engine.update_weight(107, 103, 0.05) # MAKAN menurunkan LAPAR

    engine.update_weight(101, 88, 0.2)  # MALAM -> SEMBUNYI
    engine.update_weight(101, 45, 0.05) # MALAM -> LARI rendah

    merged, pruned = engine.consolidate()
    print(f"[Init] Insting dasar tersimpan. merged={merged} pruned={pruned}")
    print(f"[Init] {engine.status()}")


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
    os.environ.setdefault("RAGP_INNATE_REGISTRY_VERSION", str(INNATE_REGISTRY_VERSION))
    reset_requested = should_reset(sys.argv[1:])
    if reset_requested and os.path.exists(STORAGE_DIR):
        shutil.rmtree(STORAGE_DIR)
        print(f"[Init] Storage lama dihapus: {STORAGE_DIR}")

    first_init = not base_file_exists(STORAGE_DIR)
    engine = RagpEngine(STORAGE_DIR)
    migration_status = engine.ensure_innate_registry(NODE_POOL_FULL)
    print(f"[Registry] {migration_status}")

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
