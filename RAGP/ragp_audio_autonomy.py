from __future__ import annotations

import argparse
import os
import time
from dataclasses import dataclass
from pathlib import Path

import numpy as np

from environment import translate
from ragp_bootstrap import innate_registry_version, node_pool_full

try:
    import sounddevice as sd
except Exception:
    sd = None

try:
    from ctn_engine import RagpEngine
except ImportError:
    print("ERROR: Modul 'ctn_engine' belum terpasang untuk interpreter ini.")
    print("Gunakan interpreter .venv dan jalankan: maturin develop --release")
    raise SystemExit(1)


@dataclass
class AudioFeatures:
    rms: float
    peak: float
    zcr: float
    spectral_centroid_hz: float
    delta_rms: float
    duration_sec: float


def capture_audio(duration_sec: float, sample_rate: int, channels: int, no_mic: bool) -> np.ndarray:
    frames = max(1, int(duration_sec * sample_rate))
    if no_mic:
        return np.zeros(frames, dtype=np.float32)
    if sd is None:
        raise RuntimeError("sounddevice tidak tersedia; install dependency audio dulu.")

    audio = sd.rec(frames, samplerate=sample_rate, channels=channels, dtype="float32")
    sd.wait()
    mono = audio[:, 0] if channels > 1 else audio.reshape(-1)
    return np.asarray(mono, dtype=np.float32)


def extract_features(samples: np.ndarray, sample_rate: int, prev_rms: float) -> AudioFeatures:
    if samples.size == 0:
        return AudioFeatures(0.0, 0.0, 0.0, 0.0, 0.0, 0.0)

    x = np.nan_to_num(samples.astype(np.float32), nan=0.0, posinf=0.0, neginf=0.0)
    rms = float(np.sqrt(np.mean(np.square(x))))
    peak = float(np.max(np.abs(x)))
    signs = np.signbit(x)
    zcr = float(np.mean(signs[1:] != signs[:-1])) if x.size > 1 else 0.0

    mag = np.abs(np.fft.rfft(x))
    freqs = np.fft.rfftfreq(x.size, d=1.0 / sample_rate)
    mag_sum = float(np.sum(mag))
    centroid = float(np.sum(freqs * mag) / mag_sum) if mag_sum > 1e-12 else 0.0

    return AudioFeatures(
        rms=rms,
        peak=peak,
        zcr=zcr,
        spectral_centroid_hz=centroid,
        delta_rms=float(max(0.0, rms - prev_rms)),
        duration_sec=float(x.size / sample_rate),
    )


def map_audio_to_nodes(features: AudioFeatures) -> list[tuple[int, float, str]]:
    out: list[tuple[int, float, str]] = []

    if features.rms >= 0.10 or features.peak >= 0.55:
        s = min(1.0, max(features.rms * 4.0, features.peak * 0.9))
        out.append((120, float(s), "loud_audio"))

    if features.delta_rms >= 0.07 or features.peak >= 0.80:
        s = min(1.0, max(features.delta_rms * 8.0, features.peak))
        out.append((140, float(s), "sudden_onset"))

    if (features.peak >= 0.85 and features.delta_rms >= 0.06) or (features.rms >= 0.16 and features.zcr >= 0.15):
        s = min(1.0, 0.5 * features.peak + 0.5 * min(1.0, features.delta_rms * 8.0))
        out.append((113, float(s), "startle_proxy"))

    if features.rms <= 0.02 and features.peak <= 0.08:
        s = min(1.0, max(0.2, (0.03 - features.rms) * 20.0))
        out.append((155, float(s), "quiet_context"))

    if features.spectral_centroid_hz >= 3200.0 and features.peak >= 0.35:
        s = min(1.0, (features.spectral_centroid_hz / 8000.0) * 0.7 + features.peak * 0.3)
        out.append((140, float(s), "high_freq_sharp"))

    merged: dict[int, tuple[float, str]] = {}
    for node_id, strength, reason in out:
        prev = merged.get(node_id)
        if prev is None or strength > prev[0]:
            merged[node_id] = (strength, reason)

    rows = [(nid, pair[0], pair[1]) for nid, pair in merged.items()]
    rows.sort(key=lambda x: x[1], reverse=True)
    return rows


def main() -> None:
    parser = argparse.ArgumentParser(description="RAGP autonomous audio stimulus loop (read/stimulus-only).")
    parser.add_argument("--duration-sec", type=float, default=1.0)
    parser.add_argument("--interval-sec", type=float, default=0.2)
    parser.add_argument("--sample-rate", type=int, default=16000)
    parser.add_argument("--channels", type=int, default=1)
    parser.add_argument("--seed-scale", type=float, default=1.0)
    parser.add_argument("--predict-limit", type=int, default=3)
    parser.add_argument("--max-loops", type=int, default=0, help="0 = infinite")
    default_storage = str(Path(__file__).resolve().parent / "ragp_storage")
    parser.add_argument("--storage-dir", default=default_storage)
    parser.add_argument("--no-mic", action="store_true", help="Use silence input for dry-run testing.")
    parser.add_argument("--force-sync", action="store_true", help="Disable async submit path and use spread_activation directly.")
    args = parser.parse_args()

    os.environ.setdefault("RAGP_INNATE_REGISTRY_VERSION", str(innate_registry_version()))
    engine = RagpEngine(args.storage_dir)
    migration_status = engine.ensure_innate_registry(node_pool_full())
    print(f"[AudioAutonomy] registry: {migration_status}")
    async_on = False
    if not args.force_sync:
        try:
            engine.start_async_runtime(None)
            async_on = True
        except Exception:
            async_on = False
    print(f"[AudioAutonomy] status: {engine.status()}")

    prev_rms = 0.0
    loops = 0
    while True:
        loops += 1
        if args.max_loops > 0 and loops > args.max_loops:
            break

        samples = capture_audio(
            duration_sec=max(0.1, min(args.duration_sec, 10.0)),
            sample_rate=max(8000, min(args.sample_rate, 48000)),
            channels=max(1, min(args.channels, 2)),
            no_mic=args.no_mic,
        )
        features = extract_features(samples, args.sample_rate, prev_rms)
        prev_rms = features.rms
        mapped = map_audio_to_nodes(features)

        if async_on and mapped:
            batch = []
            for node_id, strength, reason in mapped:
                scaled = max(0.0, min(1.0, strength * max(0.1, min(args.seed_scale, 3.0))))
                batch.append((int(node_id), float(scaled), str(reason)))
            engine.submit_stimuli(batch)
        else:
            for node_id, strength, _reason in mapped:
                scaled = max(0.0, min(1.0, strength * max(0.1, min(args.seed_scale, 3.0))))
                try:
                    engine.spread_activation(node_id, scaled)
                except ValueError as exc:
                    print(f"[AudioAutonomy][Skip] {exc}")

        top_pred: list[tuple[int, float]] = []
        if mapped:
            stim = mapped[0][0]
            ctx = [nid for nid, _, _ in mapped[1:4]]
            try:
                top_pred = engine.compute_cd(stim, ctx)[: max(1, min(args.predict_limit, 20))]
            except ValueError as exc:
                print(f"[AudioAutonomy][PredictSkip] {exc}")

        ts = time.strftime("%Y-%m-%d %H:%M:%S")
        mapped_str = ", ".join(f"{translate(nid)}:{s:.2f}" for nid, s, _ in mapped) if mapped else "-"
        pred_str = ", ".join(f"{translate(a)}:{cd:.3f}" for a, cd in top_pred) if top_pred else "-"
        print(
            f"[{ts}] loop={loops} rms={features.rms:.3f} peak={features.peak:.3f} "
            f"zcr={features.zcr:.3f} nodes=[{mapped_str}] predict=[{pred_str}]"
        )

        if args.interval_sec > 0:
            time.sleep(args.interval_sec)


if __name__ == "__main__":
    main()
