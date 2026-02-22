import time
import os
import sys

# PENTING: Anda butuh mengkompilasi file Rust dahulu menggunakan Maturin
# Buka terminal Cwd ke folder ini, ketik: `pip install maturin` lalu `maturin develop`

try:
    from ctn_engine import CtnEngine
except ImportError:
    print("ERROR: Modul 'ctn_engine' dari Rust belum dikompilasi!")
    print("Jalankan: pip install maturin && maturin develop --release")
    exit(1)

# ==============================================================================
# KAMUS SEMANTIK (LLM/Language Layer Simulator)
# Menerjemahkan node ID matematis (CTN) menjadi bahasa yang dipahami manusia.
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
    """Menerjemahkan ID Matematis ke Bahasa Manusia (Logs)"""
    return SEMANTIC_DICT.get(node_id, f"NODE_UNKNOWN_{node_id}")

class DualLogger:
    """Menggandakan perintah print() ke Terminal dan ke dalam File log.txt"""
    def __init__(self, filepath):
        self.terminal = sys.stdout
        self.log_file = open(filepath, "w", encoding="utf-8")

    def write(self, message):
        self.terminal.write(message)
        self.log_file.write(message)
        
    def flush(self):
        self.terminal.flush()
        self.log_file.flush()

def main():
    # Menentukan lokasi folder memori di hardisk
    memory_dir = os.path.join(os.getcwd(), "ctn_storage")
    if not os.path.exists(memory_dir):
        os.makedirs(memory_dir)
        
    # Mengalihkan print() agar juga tersimpan ke d:\code\RAGP5\ctn_engine_demo\ctn_storage\log.txt
    log_path = os.path.join(memory_dir, "log.txt")
    sys.stdout = DualLogger(log_path)
    
    print("="*75)
    print(" RAGP HYBRID COGNITIVE ENGINE (B-TREE INDEXING) ")
    print("="*75)
    
    # 1. INFRASTRUKTUR MEMORI (RUST)
    print("\n[Tahap Fisik: Memori / CTN Index]")
    engine = CtnEngine(memory_dir) # Instansiasi engine dengan path folder
    
    # Menyimulasikan Proses Write ke SSD dengan MURNI ID MATEMATIS
    # Saat `write_chunk` dipanggil, Rust akan OTOMATIS membuat B-Tree Peta Index (Node ID -> File Chunk)
    print(f"Mengisi memori CTN dan membentukkan Peta B-Tree...")
    # SENSOR BAHAYA (1) -> Lari (45), Sembunyi (88), dll (Disimpan di file 'c1')
    engine.write_chunk("c1", "1,45,0.90|1,88,0.70|1,12,0.20|1,23,0.50")
    # LARI (45) -> Selamat (99) (Disimpan di file 'c2')
    engine.write_chunk("c2", "45,99,0.95|88,99,0.80")
    
    # Tidak perlu lagi me-load isi file secara berantakan.
    # Biarkan Mesin B-Tree Rust yang mencarikan file-nya nanti saat prefrontal cortex butuh.

    # 2. Otak menyadari ada stimulus BAHAYA (ID: 1)
    stimulus_id = "1"
    
    print(f"\n[Tahap 3: Simulai Prefrontal Cortex (Evaluasi Solusi)]")
    print(f" -> Stimulus Masuk: ID {stimulus_id} ({translate(stimulus_id)})")
    start_time = time.perf_counter()
    
    # PYTHON TIDAK TAHU FILE MANA YANG HARUS DIBACA.
    # Saat method ini dipanggil, Rust akan:
    # 1. Mengecek B-Tree Index: "Dimana ID 1 berada?" -> Jawab: "Di c1.ctn"
    # 2. Membaca "c1.ctn" dari Hardisk ke RAM.
    # 3. Mencari substring target.
    connections = engine.get_connections(stimulus_id)
    
    end_time = time.perf_counter()
    print(f"✅ RUST B-Tree: Menemukan file, Load file ke RAM, & Menarik relasi matematis selesai dalam {(end_time - start_time) * 1000:.4f} ms")
    print(f"   => Isi Array Relasi Mentah dari Rust: {connections}\n")
    
    for receiver_id, weight in connections:
        print(f" -> [LOG KOGNITIF] Pengetahuan ditemukan: Node {receiver_id} ({translate(receiver_id)}) | Bobot Valensi: {weight}")
        
    print(f"\n[Tahap 4: Simulasi Basal Ganglia (Pengambilan Keputusan/Value-Cost)]")
    
    # Simulasi State Kognitif Saat Ini (Cost/Hambatan/Resiko)
    # Disimpan dalam bentuk ID Matematik
    state_costs = {
        "45": 0.4,   # Lari: capek, butuh energi besar
        "88": 0.1,   # Bersembunyi: energi kecil, tapi resiko ketahuan
        "12": 0.0,   # Diam: diam saja tidak ada tenaga
        "23": 0.2    # Teriak: kemungkinan menarik predator lain
    }

    best_action_id = None
    max_score = -float('inf')
    
    # Rumus Kompetisi Final Action = argmax(Value - Cost)
    for action_node_id, mem_weight in connections:
        cost = state_costs.get(action_node_id, 0.0)
        
        # Skor akhir dari keputusan logis
        final_score = mem_weight - cost
        
        human_label = translate(action_node_id)
        # Pad label agar rapi di terminal
        print(f" Evaluasi Opsi [{human_label.ljust(22)}]: Value({mem_weight:.2f}) - Cost({cost:.2f}) = Score({final_score:.2f})")
        
        # Sinyal Dopamin (Winner Takes All / Argmax)
        if final_score > max_score:
            max_score = final_score
            best_action_id = action_node_id

    print("\n" + "*"*75)
    print(f" HASIL KEPUTUSAN FINAL (EKSEKUSI MOTORIK) ")
    print(f" >>> Eksekusi Node ID: {best_action_id} <<<")
    print(f" >>> Terjemahan Aksi : {translate(best_action_id)} (Skor Kepastian: {max_score:.2f}) <<<")
    print("*"*75)
    
    # -------------------------------------------------------------
    # PENGUJIAN KEDUA: B-Tree Memilah Chunk Berbeda
    # -------------------------------------------------------------
    print("\n\n" + "="*75)
    print(" >>> SIMULASI SKENARIO 2 (CROSS-CHUNK RETRIEVAL) <<< ")
    print("="*75)
    # Setelah berlari akibat skenario 1, otak sekarang memiliki State = LARI (45)
    stimulus_id_2 = "45"
    
    print(f"\n[Tahap 3: Simulai Prefrontal Cortex (Evaluasi Lanjutan)]")
    print(f" -> Stimulus Masuk: ID {stimulus_id_2} ({translate(stimulus_id_2)})")
    start_time_2 = time.perf_counter()
    
    # RUST B-TREE ACTION:
    # B-Tree akan melihat ID 45. Ia tahu ID 45 ada di "c2.ctn".
    # Ia secara instan melompati c1.ctn, membongkar "c2.ctn", lalu mencari relasinya.
    connections_2 = engine.get_connections(stimulus_id_2)
    
    end_time_2 = time.perf_counter()
    print(f"✅ RUST B-Tree: Transisi ke Chunk 'c2', Load file, & Ekstrak relasi dalam {(end_time_2 - start_time_2) * 1000:.4f} ms")
    print(f"   => Isi Array Relasi Mentah dari Rust: {connections_2}\n")
    
    for receiver_id, weight in connections_2:
        print(f" -> [LOG KOGNITIF] Pengetahuan ditemukan: Node {receiver_id} ({translate(receiver_id)}) | Bobot Valensi: {weight}")

    # -------------------------------------------------------------
    # PENGUJIAN KETIGA: NEUROPLASTICITY (LONG-TERM POTENTIATION)
    # Simulasi Pembelajaran dari Kesalahan (Updating Weights)
    # -------------------------------------------------------------
    print("\n\n" + "="*75)
    print(" >>> SIMULASI SKENARIO 3 (PEMBELAJARAN / NEUROPLASTICITY) <<< ")
    print("="*75)
    
    # RAGP sebelumnya menetapkan aksi SEMBUNYI (88) sebagai pemenang di Skenario 1.
    # Mari kita asumsikan setelah RAGP bersembunyi, ternyata PREDATOR TETAP MENEJMUKANNYA!
    # Terjadi semacam rasa "sakit" (Pain Signal).
    print("[Evaluasi Pasca-Aksi (Feedback Loop)]")
    print(f"Sistem mengeksekusi: {translate(best_action_id)} ({best_action_id})")
    print("HASIL: GAGAL! Predator tetap menyerang (Negative Reward / Pain Signal Diterima)")
    
    # Karena gagal, kita harus MENGHUKUM memori ini agar besok-besok tidak dipakai lagi.
    # Ingatan awal: BAHAYA (1) -> SEMBUNYI (88) punya weight 0.70.
    # Kita akan turunkan drastis weight-nya menjadi 0.10.
    old_weight = 0.70
    new_weight = 0.10
    
    print(f"\n[Memicu Long-Term Potentiation (LTP) ke Rust Engine]")
    print(f"Menurunkan bobot Relasi 1 -> 88 dari {old_weight} menjadi {new_weight}...")
    
    start_time_3 = time.perf_counter()
    # Panggil fungsi Rust untuk menulis ulang fisik memori di hardisk
    engine.update_weight("1", "88", new_weight)
    end_time_3 = time.perf_counter()
    
    print(f"✅ RUST LTP: Mengubah memori RAM & Menulis ulang file .ctn di SSD selesai dalam {(end_time_3 - start_time_3) * 1000:.4f} ms")
    
    print("\n[Membuktikan Perubahan Memori Fisik]")
    print("Mari kita panggil ulang Stimulus BAHAYA (ID: 1) untuk melihat ingatan barunya:")
    connections_after_learning = engine.get_connections("1")
    for receiver_id, weight in connections_after_learning:
        indicator = " (<<<< UPDATE BARU!)" if receiver_id == "88" else ""
        print(f" -> Pengetahuan: Node {receiver_id} ({translate(receiver_id)}) | Bobot Valensi: {weight}{indicator}")

if __name__ == "__main__":
    main()
