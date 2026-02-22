import time
import os
import sys
import psutil

try:
    from ctn_engine import CtnEngine
except ImportError:
    print("ERROR: Modul 'ctn_engine' dari Rust belum dikompilasi!")
    print("Jalankan: pip install maturin && maturin develop --release")
    exit(1)

# ==============================================================================
# KAMUS SEMANTIK (LLM/Language Layer Simulator)
# ==============================================================================
SEMANTIC_DICT = {
    "1":  "SENSOR_BAHAYA",
    "45": "LARI_SEKARANG",
    "88": "BERSEMBUNYI",
    "12": "DIAM_TERPAKU",
    "23": "TERIAK_MINTA_TOLONG",
    "99": "SELAMAT_DARI_BAHAYA",
}

def translate(node_id):
    return SEMANTIC_DICT.get(node_id, f"NODE_UNKNOWN_{node_id}")

class DualLogger:
    def __init__(self, filepath):
        self.terminal = sys.stdout
        self.log_file = open(filepath, "w", encoding="utf-8")
    def write(self, message):
        self.terminal.write(message)
        self.log_file.write(message)
    def flush(self):
        self.terminal.flush()
        self.log_file.flush()

# ==============================================================================
# HIPPOCAMPUS BUFFER (RAM Layer)
# Format: { (sender_id, receiver_id): {"accumulated_reward": float, "count": int, "peak": float} }
# ==============================================================================
hippocampus_buffer = {}

def record_experience(sender_id, receiver_id, reward):
    """Merekam pengalaman baru ke Hippocampus Buffer (RAM)."""
    key = (sender_id, receiver_id)
    if key not in hippocampus_buffer:
        hippocampus_buffer[key] = {"accumulated_reward": 0.0, "count": 0, "peak": 0.0}
    
    entry = hippocampus_buffer[key]
    entry["accumulated_reward"] += reward
    entry["count"] += 1
    if abs(reward) > abs(entry["peak"]):
        entry["peak"] = reward

def check_ram_threshold(limit_percent=75.0):
    """Mengecek apakah RAM sudah melewati batas fisik - threshold biologis RAGP."""
    used = psutil.virtual_memory().percent
    return used > limit_percent, used

def rescorla_wagner(old_weight, reward, count=1):
    """
    Rumus Rescorla-Wagner: V_new = V_old + α × (R - V_old)
    α = 1/count (Decaying Learning Rate):
      - Episode 1: α = 1.0 (belajar penuh, belum ada referensi)
      - Episode 5: α = 0.2 (sudah punya 4 pengalaman sebelumnya)
      - Episode 10: α = 0.1 (makin stabil, susah digoyahkan)
    """
    learning_rate = 1.0 / count  # decaying: makin banyak pengalaman, makin kecil α
    delta = learning_rate * (reward - old_weight)
    new_weight = old_weight + delta
    return max(0.0, min(1.0, new_weight))  # Clamp ke [0.0, 1.0]

def consolidate(engine, connections_map):
    """
    Proses konsolidasi Hippocampus → Neokorteks.
    Menggunakan adaptive mean threshold berdasarkan riset:
    "Emotional arousal tags memories as more significant than average."
    Hanya memori dengan |peak| > rata-rata sinyal hari ini yang dikonsolidasi.
    """
    print("\n[KONSOLIDASI] Hippocampus → Neokorteks (SSD) dimulai...")
    consolidated_count = 0

    if not hippocampus_buffer:
        print("[KONSOLIDASI] Buffer kosong, tidak ada yang perlu dikonsolidasi.")
        return

    # Hitung rata-rata kekuatan sinyal dari semua entri di buffer (Adaptive Threshold)
    mean_signal = sum(abs(e["peak"]) for e in hippocampus_buffer.values()) / len(hippocampus_buffer)
    print(f"  [Adaptive Threshold] Rata-rata sinyal hari ini: {mean_signal:.3f}")

    for (sender_id, receiver_id), entry in hippocampus_buffer.items():
        acc = entry["accumulated_reward"]
        peak = entry["peak"]

        # Filter adaptif: hanya yang di atas rata-rata yang lolos ke Neokorteks
        if abs(peak) > mean_signal:
            old_weight = connections_map.get((sender_id, receiver_id), 0.5)
            new_weight = rescorla_wagner(old_weight, reward=acc, count=entry["count"])

            print(f"  [KONSOLIDASI] {sender_id}({translate(sender_id)}) → "
                  f"{receiver_id}({translate(receiver_id)}): "
                  f"weight {old_weight:.2f} → {new_weight:.2f} "
                  f"(peak={peak:.2f} > mean={mean_signal:.2f})")

            engine.update_weight(sender_id, receiver_id, new_weight)
            consolidated_count += 1
        else:
            print(f"  [BUANG] {sender_id}→{receiver_id}: sinyal lemah "
                  f"(peak={peak:.2f} ≤ mean={mean_signal:.2f}) — tidak dikonsolidasi")

    print(f"[KONSOLIDASI] Selesai. {consolidated_count}/{len(hippocampus_buffer)} "
          f"entri berhasil ditulis ke Neokorteks (.ctn).")

def clear_hippocampus():
    """Membersihkan staging buffer RAM setelah konsolidasi (bangun tidur)."""
    hippocampus_buffer.clear()
    print("[HIPPOCAMPUS] Buffer RAM dibersihkan (RAGP 'bangun tidur').\n")

# ==============================================================================
# MAIN SIMULATION
# ==============================================================================
def main():
    memory_dir = os.path.join(os.getcwd(), "ctn_storage")
    if not os.path.exists(memory_dir):
        os.makedirs(memory_dir)

    log_path = os.path.join(memory_dir, "log.txt")
    sys.stdout = DualLogger(log_path)

    print("="*75)
    print(" RAGP COGNITIVE ENGINE — PHASE 8: HIPPOCAMPUS / NEOCORTEX ")
    print("="*75)

    # --- NEOKORTEKS: Inisialisasi file .ctn (SSD) ---
    engine = CtnEngine(memory_dir)
    engine.write_chunk("c1", "1,45,0.90|1,88,0.70|1,12,0.20|1,23,0.50")
    engine.write_chunk("c2", "45,99,0.95|88,99,0.80")
    print("[Neokorteks] File .ctn berhasil dimuat ke B-Tree index Rust.\n")

    # Peta weight referensi (untuk Rescorla-Wagner lookup)
    known_weights = {
        ("1", "45"): 0.90, ("1", "88"): 0.70,
        ("1", "12"): 0.20, ("1", "23"): 0.50,
    }

    # --- SIMULASI MULTI-PENGALAMAN ---
    print("="*75)
    print(" SIMULASI PENGALAMAN BERULANG (5 Episode) ")
    print("="*75)

    # Skenario: 5x ketemu bahaya, setiap kali mencoba SEMBUNYI (88)
    # Hasil bervariasi: kadang berhasil, kadang gagal
    episodes = [
        ("1", "88", -0.60, "GAGAL - ketahuan predator"),
        ("1", "88", -0.50, "GAGAL - predator mencium bau"),
        ("1", "88",  0.30, "BERHASIL - sembunyi di balik batu"),
        ("1", "88", -0.70, "GAGAL - posisi sembunyi buruk"),
        ("1", "45",  0.80, "BERHASIL - lari dan selamat"),
    ]

    for i, (sender, receiver, reward, desc) in enumerate(episodes, 1):
        print(f"\n[Episode {i}] Stimulus: {translate(sender)} → "
              f"Aksi: {translate(receiver)}")
        print(f"  Hasil: {desc} | Reward Signal: {reward:+.2f}")

        # Rekam ke Hippocampus Buffer (RAM)
        record_experience(sender, receiver, reward)

        # Cek apakah RAM sudah melewati threshold fisik
        over_limit, ram_pct = check_ram_threshold(limit_percent=75.0)
        print(f"  [RAM Monitor] Penggunaan RAM: {ram_pct:.1f}% "
              f"{'(THRESHOLD TERLAMPAUI → konsolidasi!)' if over_limit else '(masih aman)'}")

        if over_limit:
            consolidate(engine, known_weights)
            clear_hippocampus()

    # Konsolidasi akhir setelah semua episode selesai (simulasi "waktu tidur")
    print("\n" + "="*75)
    print(" WAKTU TIDUR — KONSOLIDASI AKHIR ")
    print("="*75)
    consolidate(engine, known_weights)
    clear_hippocampus()

    # Buktikan perubahan di Neokorteks (SSD)
    print("\n[Verifikasi Neokorteks] Membaca kembali memori BAHAYA (ID: 1) dari .ctn:")
    updated_connections = engine.get_connections("1")
    for receiver_id, weight in updated_connections:
        print(f"  → {receiver_id} ({translate(receiver_id)}): weight = {weight:.2f}")

if __name__ == "__main__":
    main()
