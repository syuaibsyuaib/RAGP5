from __future__ import annotations

import json
import os
from pathlib import Path
from typing import Any, Iterable


DEFAULT_REGISTRY_VERSION = 1
DEFAULT_NODE_MAX = 109
DEFAULT_SEED_LINKS: tuple[tuple[int, int, float], ...] = (
    (1, 45, 0.3),     # BAHAYA -> LARI
    (1, 88, 0.2),     # BAHAYA -> SEMBUNYI
    (1, 12, 0.1),     # BAHAYA -> DIAM
    (103, 106, 0.3),  # LAPAR -> CARI_MAKAN
    (103, 107, 0.2),  # LAPAR -> MAKAN
    (100, 108, 0.3),  # LELAH -> ISTIRAHAT
    (100, 109, 0.2),  # LELAH -> TIDUR
    (104, 109, 0.3),  # SAKIT -> TIDUR
    (104, 108, 0.2),  # SAKIT -> ISTIRAHAT
    (45, 100, 0.15),  # Cost LARI
    (88, 100, 0.05),  # Cost SEMBUNYI
    (12, 100, 0.02),  # Cost DIAM
    (106, 100, 0.10), # Cost CARI_MAKAN
    (107, 103, 0.05), # MAKAN menurunkan LAPAR
    (101, 88, 0.2),   # MALAM -> SEMBUNYI
    (101, 45, 0.05),  # MALAM -> LARI rendah
)
DEFAULT_BOOTSTRAP: dict[str, Any] = {
    "registry_version": DEFAULT_REGISTRY_VERSION,
    "node_max": DEFAULT_NODE_MAX,
    "seed_links": [
        {"sender": s, "receiver": r, "weight": w}
        for s, r, w in DEFAULT_SEED_LINKS
    ],
}


def _bootstrap_config_path() -> Path:
    return Path(
        os.environ.get(
            "RAGP_BOOTSTRAP_CONFIG",
            str(Path(__file__).with_name("ragp_bootstrap_config.json")),
        )
    )


def _safe_int(value: Any, default: int) -> int:
    try:
        return int(value)
    except (TypeError, ValueError):
        return default


def _safe_float(value: Any, default: float) -> float:
    try:
        return float(value)
    except (TypeError, ValueError):
        return default


def _load_bootstrap_config() -> dict[str, Any]:
    path = _bootstrap_config_path()
    if not path.exists():
        return DEFAULT_BOOTSTRAP

    try:
        data = json.loads(path.read_text(encoding="utf-8-sig"))
    except Exception:
        return DEFAULT_BOOTSTRAP

    if not isinstance(data, dict):
        return DEFAULT_BOOTSTRAP

    merged = dict(DEFAULT_BOOTSTRAP)
    merged.update(data)
    return merged


def innate_registry_version() -> int:
    cfg = _load_bootstrap_config()
    env_value = os.environ.get("RAGP_INNATE_REGISTRY_VERSION")
    if env_value is not None:
        return max(1, _safe_int(env_value, DEFAULT_REGISTRY_VERSION))
    return max(1, _safe_int(cfg.get("registry_version"), DEFAULT_REGISTRY_VERSION))


def node_pool_full() -> list[int]:
    cfg = _load_bootstrap_config()
    max_node_env = os.environ.get("RAGP_NODE_MAX")
    if max_node_env is not None:
        max_node = max(1, _safe_int(max_node_env, DEFAULT_NODE_MAX))
    else:
        max_node = max(1, _safe_int(cfg.get("node_max"), DEFAULT_NODE_MAX))
    return list(range(1, max_node + 1))


def seed_links() -> list[tuple[int, int, float]]:
    cfg = _load_bootstrap_config()
    raw_links = cfg.get("seed_links", DEFAULT_BOOTSTRAP["seed_links"])
    if not isinstance(raw_links, list):
        raw_links = DEFAULT_BOOTSTRAP["seed_links"]

    out: list[tuple[int, int, float]] = []
    for item in raw_links:
        if not isinstance(item, dict):
            continue
        sender = _safe_int(item.get("sender"), 0)
        receiver = _safe_int(item.get("receiver"), 0)
        weight = _safe_float(item.get("weight"), 0.0)
        if sender <= 0 or receiver <= 0:
            continue
        out.append((sender, receiver, max(0.0, min(1.0, weight))))

    if out:
        return out
    return list(DEFAULT_SEED_LINKS)


def seed_initial_knowledge(engine) -> None:
    print("[Init] Menanamkan insting dasar...")
    links: Iterable[tuple[int, int, float]] = seed_links()
    for sender, receiver, weight in links:
        engine.update_weight(sender, receiver, weight)

    merged, pruned = engine.consolidate()
    print(f"[Init] Insting dasar tersimpan. merged={merged} pruned={pruned}")
    print(f"[Init] {engine.status()}")
