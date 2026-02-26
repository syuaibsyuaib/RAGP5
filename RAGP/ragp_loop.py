from __future__ import annotations

from environment import VirtualEnvironment, translate

# Action nodes (int, aligned with Rust u64 API)
AKSI_NODES = {45, 88, 12, 106, 107, 108, 109}

# Stimulus priority (highest first)
PRIORITAS_STIMULUS = [104, 1, 103, 100]
KONTEKS_NODES = {101, 102, 105}


def _engine_async_on(engine) -> bool:
    try:
        metrics = engine.get_async_metrics()
    except Exception:
        return False
    return bool(metrics.get("async_on", False))


def run_survival_loop(
    engine,
    hippocampus: dict,
    konsolidasi_fn,
    max_steps: int = 200,
    seed: int | None = None,
    verbose: bool = True,
):
    """
    Main survival loop.

    Engine contract:
    - compute_cd(stimulus: int, context: list[int]) -> list[tuple[int, float]]
    - spread_activation(seed_node: int, seed_strength: float)
    - form_synapses_from_window() -> int
    """
    env = VirtualEnvironment(seed=seed)

    _log_header()
    step = 0
    while step < max_steps and not env.gugur:
        step += 1

        # Environment provides string sensor IDs; convert to int for Rust engine.
        sensors = [int(s) for s in env.get_active_sensors()]

        if _engine_async_on(engine):
            batch = [(int(sensor), 1.0, "survival_loop") for sensor in sensors]
            if batch:
                engine.submit_stimuli(batch)
        else:
            for sensor in sensors:
                engine.spread_activation(sensor, 1.0)

        stimulus, context = _parse_sensors(sensors)
        if stimulus is None:
            aksi_id = 108
            alasan = "no urgent stimulus -> ISTIRAHAT"
        else:
            hasil_cd = engine.compute_cd(stimulus, context)
            hasil_cd = [(a, cd) for a, cd in hasil_cd if a in AKSI_NODES]
            if not hasil_cd:
                aksi_id = 108
                alasan = f"no valid action for stimulus {translate(stimulus)}"
            else:
                aksi_id = hasil_cd[0][0]
                alasan = (
                    f"stimulus={translate(stimulus)} "
                    f"context={[translate(c) for c in context]} "
                    f"Cd={hasil_cd[0][1]:.3f}"
                )

        result = env.apply_action(str(aksi_id))
        reward = float(result["reward"])

        formed = int(engine.form_synapses_from_window())

        if stimulus is not None and reward != 0.0:
            _rekam_hippocampus(hippocampus, stimulus, aksi_id, reward)

        if verbose:
            _log_step(step, sensors, aksi_id, result, alasan, formed)

        if step % 20 == 0 and not env.gugur:
            if verbose:
                print(f"\n{'=' * 70}")
                print(f" [Tidur] Langkah {step} - Konsolidasi dimulai")
                print(f"{'=' * 70}")
            konsolidasi_fn(hippocampus, engine, verbose=verbose)
            hippocampus.clear()
            if verbose:
                print(" [Bangun] Hippocampus dibersihkan.\n")

    _log_footer(env, step)
    return env


def _parse_sensors(sensors: list[int]) -> tuple[int | None, list[int]]:
    stimulus = None
    context: list[int] = []

    for p in PRIORITAS_STIMULUS:
        if p in sensors:
            stimulus = p
            break

    for s in sensors:
        if s in KONTEKS_NODES:
            context.append(s)

    return stimulus, context


def _rekam_hippocampus(hippocampus: dict, stimulus: int, aksi_id: int, reward: float):
    key = (stimulus, aksi_id)
    if key not in hippocampus:
        hippocampus[key] = {"acc": 0.0, "count": 0, "peak": 0.0}

    entry = hippocampus[key]
    entry["acc"] += reward
    entry["count"] += 1
    if abs(reward) > abs(entry["peak"]):
        entry["peak"] = reward


def _log_header():
    print("\n" + "=" * 70)
    print(" RAGP5 - SURVIVAL LOOP")
    print("=" * 70)
    print(f"{'Step':>4} | {'Sensors':<32} | {'Aksi':<16} | {'H':>4} {'L':>4} {'Lt':>4} | {'Reward':>7}")
    print("-" * 70)


def _log_footer(env: VirtualEnvironment, step: int):
    print("\n" + "=" * 70)
    status = "GUGUR" if env.gugur else "HIDUP"
    print(f" Selesai setelah {step} langkah | Status: {status}")
    print(f" Health={env.health:.2f} Lapar={env.lapar:.2f} Lelah={env.lelah:.2f}")
    print("=" * 70)


def _log_step(step: int, sensors: list[int], aksi_id: int, result: dict, alasan: str, formed: int):
    sensor_names = [translate(s) for s in sensors]
    aksi_name = translate(aksi_id)
    h = result["health"]
    l = result["lapar"]
    lt = result["lelah"]
    rw = result["reward"]
    formed_msg = f" [+{formed}syn]" if formed > 0 else ""
    print(
        f"{step:>4} | {str(sensor_names):<32} | {aksi_name:<16} | "
        f"{h:>4.2f} {l:>4.2f} {lt:>4.2f} | {rw:>+7.3f}{formed_msg}"
    )
