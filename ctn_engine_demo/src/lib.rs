use pyo3::prelude::*;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::PathBuf;

/// Mesin Parser CTN Bawah Tanah (Rust)
/// Memproses dan Menyimpan Ingatan Graf Jaringan Saraf
#[pyclass]
struct CtnEngine {
    // Folder tempat menyimpan file fisik .ctn
    storage_path: PathBuf,

    // ==========================================
    // TAHAP 6: B-TREE INDEXING
    // Peta yang memberi tahu sistem di file mana sebuah ID berada.
    // Kunci: ID Pengirim (Matematis), Nilai: Nama File Chunk (c1, c2, dst)
    // BTreeMap pada Rust secara otomatis diurutkan (O(log n) search time).
    // ==========================================
    index: BTreeMap<u64, String>,

    // Kunci: ID Chunk (Leaf), Nilai: String CTN 1 baris
    // Ini adalah Working Memory (RAM)
    loaded_chunks: HashMap<String, String>,
}

#[pymethods]
impl CtnEngine {
    #[new]
    fn new(storage_dir: String) -> Self {
        let path = PathBuf::from(storage_dir);
        // Buat folder jika belum ada
        if !path.exists() {
            fs::create_dir_all(&path).expect("Gagal membuat direktori storage CTN");
        }

        CtnEngine {
            storage_path: path,
            index: BTreeMap::new(),
            loaded_chunks: HashMap::new(),
        }
    }

    /// Menyimpan Chunk Data CTN baru ke Hardisk & Meng-update B-Tree Index
    fn write_chunk(&mut self, chunk_id: String, ctn_data: String) {
        // 1. Tulis fisik SSD
        let file_path = self.storage_path.join(format!("{}.ctn", chunk_id));
        fs::write(&file_path, &ctn_data).expect("Gagal menge-save file CTN ke hardisk");

        // 2. Parsel String untuk mencari ID unik yang ada di chunk ini
        // Format CTN: "pengirim,penerima,weight|..."
        for triplet in ctn_data.split('|') {
            let parts: Vec<&str> = triplet.split(',').collect();
            if parts.len() == 3 {
                // Konversi pengirim ke angka (u64)
                if let Ok(sender_id) = parts[0].parse::<u64>() {
                    // Masukkan ke B-Tree Index: "Jika cari ID ini, buka file chunk_id"
                    self.index.insert(sender_id, chunk_id.clone());
                }
            }
        }

        // 3. (Opsional) Langsung load ke RAM setelah kutulis
        self.loaded_chunks.insert(chunk_id, ctn_data);
    }

    /// (Internal) Membaca Chunk spesifik dari Hardisk ke RAM
    fn load_chunk_by_id(&mut self, chunk_id: &str) -> bool {
        let file_path = self.storage_path.join(format!("{}.ctn", chunk_id));
        if let Ok(content) = fs::read_to_string(file_path) {
            self.loaded_chunks.insert(chunk_id.to_string(), content);
            true
        } else {
            false
        }
    }

    /// SMART QUERY ROUTING (B-TREE SEARCH + LAZY LOADING)
    /// Mencari pengirim dengan cepat dengan melihat Peta B-Tree terlebih dahulu
    fn get_connections(&mut self, sender_id_str: &str) -> Vec<(String, f64)> {
        let mut results = Vec::new();

        // 1. Konversi text input ke Angka untuk pencarian B-Tree
        let sender_id: u64 = match sender_id_str.parse() {
            Ok(val) => val,
            Err(_) => return results, // Invalid ID
        };

        // 2. Cari di Peta B-Tree (O(log n) speed)
        // Di file mana si `sender_id` ini berada?
        let target_chunk = match self.index.get(&sender_id) {
            Some(chunk_name) => chunk_name.clone(),
            None => {
                // Tidak ada di dalam Index.
                return results;
            }
        };

        // 3. Lazy Loading - Cek apakah file ini sudah ada di Working Memory (RAM)?
        if !self.loaded_chunks.contains_key(&target_chunk) {
            // Belum ada! Berarti harus panggil petugas untuk ambil di Hardisk.
            self.load_chunk_by_id(&target_chunk);
        }

        // 4. Ekstrak data substring secara brutal O(N) PADA CHUNK SPESIFIK SAJA
        if let Some(data) = self.loaded_chunks.get(&target_chunk) {
            let search_prefix = format!("{},", sender_id_str);
            for triplet in data.split('|') {
                if triplet.starts_with(&search_prefix) {
                    let parts: Vec<&str> = triplet.split(',').collect();
                    if parts.len() == 3 {
                        let receiver = parts[1].to_string();
                        if let Ok(weight) = parts[2].parse::<f64>() {
                            results.push((receiver, weight));
                        }
                    }
                }
            }
        }

        results
    }

    /// TAHAP 9: COMPETITION DEGREE (BASAL GANGLIA)
    /// Menghitung Cd = (value × opportunity) / cost untuk setiap aksi
    /// dari stimulus tertentu, dengan mempertimbangkan konteks aktif.
    /// Return: Vec<(aksi_id, Cd)> diurutkan dari Cd tertinggi.
    fn compute_cd(&mut self, stimulus: &str, context: Vec<String>) -> Vec<(String, f64)> {
        let mut cd_results: Vec<(String, f64)> = Vec::new();

        // 1. Ambil semua aksi dari stimulus (value)
        let actions = self.get_connections(stimulus);
        if actions.is_empty() {
            return cd_results;
        }

        for (action_id, value) in &actions {

            // 2. Ambil cost: aksi → resource node (ambil weight tertinggi)
            let cost_connections = self.get_connections(action_id);
            let cost = if cost_connections.is_empty() {
                1.0 // Tidak ada data cost = asumsikan maksimal (paling mahal)
            } else {
                let total: f64 = cost_connections.iter().map(|(_, w)| w).sum();
                total / cost_connections.len() as f64
            };

            // 3. Ambil opportunity: context → aksi (rata-rata dari semua konteks aktif)
            let mut opp_weights: Vec<f64> = Vec::new();
            for ctx in &context {
                let ctx_connections = self.get_connections(ctx);
                for (target, w) in &ctx_connections {
                    if target == action_id {
                        opp_weights.push(*w);
                    }
                }
            }
            let opportunity = if opp_weights.is_empty() {
                0.5 // Tidak ada data opportunity = netral
            } else {
                opp_weights.iter().sum::<f64>() / opp_weights.len() as f64
            };

            // 4. Hitung Cd
            let cd = if cost == 0.0 {
                f64::MAX // Cost nol = gratis = Cd tak terhingga
            } else {
                (value * opportunity) / cost
            };

            cd_results.push((action_id.clone(), cd));
        }

        // 5. Urutkan dari Cd tertinggi (pemenang kompetisi)
        cd_results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        cd_results
    }

    /// TAHAP 7: NEUROPLASTICITY (LONG-TERM POTENTIATION)
    /// Mengubah bobot valensi dari memori yang sudah ada, atau menambahkan memori baru,
    /// dan langsung me-rewrite ke Hardisk.
    fn update_weight(&mut self, sender_id_str: &str, receiver_id_str: &str, new_weight: f64) {
        // Coba parsing ke u64
        let sender_id: u64 = match sender_id_str.parse() {
            Ok(v) => v,
            Err(_) => return, // Invalid ID
        };

        // Cari tahu di mana chunk-nya
        let target_chunk = match self.index.get(&sender_id) {
            Some(chunk) => chunk.clone(),
            None => {
                // Skenario pembuatan memori super baru (belum kita support sepenuhnya di prototype ini
                // tanpa mekanisme alokasi nama file otomatis). Abaikan dulu.
                return;
            }
        };

        // Pastikan load ke RAM
        if !self.loaded_chunks.contains_key(&target_chunk) {
            if !self.load_chunk_by_id(&target_chunk) {
                return; // gagal load
            }
        }

        let mut updated_ctn_string = String::new();
        let mut modified = false;

        // Modifikasi string panjang di RAM
        if let Some(ctn_data) = self.loaded_chunks.get(&target_chunk) {
            let mut new_triplets = Vec::new();
            let target_prefix = format!("{},{},", sender_id_str, receiver_id_str);

            for triplet in ctn_data.split('|') {
                if triplet.starts_with(&target_prefix) {
                    // Update yang sudah ada
                    new_triplets.push(format!(
                        "{},{},{}",
                        sender_id_str, receiver_id_str, new_weight
                    ));
                    modified = true;
                } else {
                    // Pertahankan yang sudah ada
                    new_triplets.push(triplet.to_string());
                }
            }

            // Jika relasi ini belum pernah ada (tapi ID sedernya ada di file ini), tambahkan ke ekor
            if !modified {
                new_triplets.push(format!(
                    "{},{},{}",
                    sender_id_str, receiver_id_str, new_weight
                ));
            }

            updated_ctn_string = new_triplets.join("|");
        }

        // Tulis (Rewrite) kembali ke Hardisko & RAM
        if !updated_ctn_string.is_empty() {
            let file_path = self.storage_path.join(format!("{}.ctn", target_chunk));
            fs::write(&file_path, &updated_ctn_string).expect("Gagal menulis ulang file CTN");
            self.loaded_chunks.insert(target_chunk, updated_ctn_string);
        }
    }
}

#[pymodule]
fn ctn_engine(_py: Python, m: &PyModule) -> PyResult<()> {
    m.add_class::<CtnEngine>()?;
    Ok(())
}
