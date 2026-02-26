# =============================================================================
# RAGP5 - VIRTUAL ENVIRONMENT (Minecraft-inspired Survival)
# =============================================================================
# Prinsip:
#   - Health adalah satu-satunya indikator survival (root node SAKIT)
#   - Lapar dan Lelah turun setiap aksi dilakukan
#   - Lapar=0 atau Lelah=0 menggerus Health
#   - Bahaya muncul dinamis, tidak ekstrem
#   - Health=0 → RAGP gugur
# =============================================================================

import random
import time
import json
import os
from pathlib import Path

# -----------------------------------------------------------------------------
# SEMANTIC DICT (node ID → nama)
# -----------------------------------------------------------------------------
SENSOR_NODES = {
    "1":   "SENSOR_BAHAYA",
    "12":  "DIAM_TERPAKU",
    "23":  "TERIAK_MINTA_TOLONG",  # ada di pool tapi tidak aktif
    "45":  "LARI_SEKARANG",
    "88":  "BERSEMBUNYI",
    "99":  "SELAMAT",
    "100": "SENSOR_LELAH",
    "101": "MALAM",
    "102": "SEMAK_ADA",
    "103": "SENSOR_LAPAR",
    "104": "SENSOR_SAKIT",
    "105": "SIANG",
    "106": "CARI_MAKAN",
    "107": "MAKAN",
    "108": "ISTIRAHAT",
    "109": "TIDUR",
    "110": "SENSOR_CO2_DISTRESS",
    "111": "SENSOR_HAUS",
    "112": "SENSOR_STRES_TERMAL",
    "113": "SENSOR_STARTLE",
    "114": "SENSOR_O2_RENDAH",
    "120": "KONTEKS_SUARA_KERAS",
    "121": "KONTEKS_PANAS_EKSTREM",
    "122": "KONTEKS_DINGIN_EKSTREM",
    "123": "KONTEKS_MULUT_KERING",
    "124": "KONTEKS_NAPAS_BERAT",
    "130": "AKSI_CARI_UDARA",
    "131": "AKSI_CARI_MINUM",
    "132": "AKSI_MENDINGIN",
    "133": "AKSI_MENGHANGAT",
    "134": "AKSI_FREEZE_ORIENT",
    "135": "AKSI_MINTA_BANTUAN",
}


def _bootstrap_config_path() -> Path:
    return Path(
        os.environ.get(
            "RAGP_BOOTSTRAP_CONFIG",
            str(Path(__file__).with_name("ragp_bootstrap_config.json")),
        )
    )


def _load_dynamic_semantics() -> dict[str, str]:
    path = _bootstrap_config_path()
    if not path.exists():
        return {}
    try:
        obj = json.loads(path.read_text(encoding="utf-8-sig"))
    except Exception:
        return {}
    if not isinstance(obj, dict):
        return {}
    raw = obj.get("node_semantics", {})
    if not isinstance(raw, dict):
        return {}

    out: dict[str, str] = {}
    for key, value in raw.items():
        if value is None:
            continue
        out[str(key)] = str(value)
    return out


SENSOR_NODES.update(_load_dynamic_semantics())

def translate(node_id) -> str:
    """Terima str atau int/u64, return nama semantik."""
    return SENSOR_NODES.get(str(node_id), f"NODE_{node_id}")

# -----------------------------------------------------------------------------
# KONSTANTA
# -----------------------------------------------------------------------------
# Threshold sensor mulai firing
THRESHOLD_LAPAR   = 0.4   # lapar < 0.4 → SENSOR_LAPAR aktif
THRESHOLD_LELAH   = 0.3   # lelah < 0.3 → SENSOR_LELAH aktif
THRESHOLD_SAKIT   = 0.7   # health < 0.7 → SENSOR_SAKIT aktif

# Seberapa besar lapar/lelah menggerus health saat mencapai 0
DRAIN_LAPAR       = 0.05  # per aksi saat lapar=0
DRAIN_LELAH       = 0.03  # per aksi saat lelah=0

# Cost tiap aksi terhadap lapar dan lelah
ACTION_COST = {
    "45":  {"lapar": 0.12, "lelah": 0.15},  # LARI: paling boros
    "88":  {"lapar": 0.05, "lelah": 0.04},  # BERSEMBUNYI: hemat
    "12":  {"lapar": 0.02, "lelah": 0.01},  # DIAM: sangat hemat
    "106": {"lapar": 0.08, "lelah": 0.10},  # CARI_MAKAN: butuh tenaga
    "107": {"lapar": 0.00, "lelah": 0.02},  # MAKAN: isi lapar
    "108": {"lapar": 0.01, "lelah": 0.00},  # ISTIRAHAT: pulihkan lelah
    "109": {"lapar": 0.02, "lelah": 0.00},  # TIDUR: pulihkan lelah lebih
}

# Efek aksi terhadap recovery
ACTION_RECOVERY = {
    "107": {"lapar": 0.40},               # MAKAN: isi lapar
    "108": {"lelah": 0.25},               # ISTIRAHAT: pulihkan lelah
    "109": {"lelah": 0.60, "health": 0.10}, # TIDUR: pulihkan lelah + health
}

# Bahaya
BAHAYA_MIN_INTERVAL = 5    # minimum aksi sebelum bahaya berikutnya
BAHAYA_MAX_INTERVAL = 15   # maximum aksi sebelum bahaya berikutnya
BAHAYA_DAMAGE_MIN   = 0.10 # kerusakan minimum per serangan
BAHAYA_DAMAGE_MAX   = 0.25 # kerusakan maksimum per serangan

# Siang/malam cycle (dalam jumlah aksi)
CYCLE_PANJANG       = 20   # 20 aksi = 1 hari penuh
MALAM_MULAI         = 12   # aksi ke-12 = mulai malam

# -----------------------------------------------------------------------------
# KELAS UTAMA
# -----------------------------------------------------------------------------
class VirtualEnvironment:
    """
    Lingkungan virtual survival berbasis prinsip Minecraft.
    Output utama: list node ID yang aktif → masuk ke RAGP sebagai stimulus.
    """

    def __init__(self, seed: int = None):
        if seed is not None:
            random.seed(seed)

        # State agent
        self.health  = 1.0
        self.lapar   = 1.0
        self.lelah   = 1.0

        # State dunia
        self.aksi_count        = 0   # total aksi dilakukan
        self.cycle_pos         = 0   # posisi dalam siang/malam cycle
        self.bahaya_aktif      = False
        self.aksi_sejak_bahaya = 0
        self.interval_bahaya   = self._next_interval()
        self.semak_ada         = random.random() > 0.5  # 50% chance ada semak

        # Log
        self.log = []
        self.gugur = False

    # -------------------------------------------------------------------------
    # INTERNAL HELPERS
    # -------------------------------------------------------------------------
    def _next_interval(self) -> int:
        """Tentukan kapan bahaya berikutnya muncul."""
        return random.randint(BAHAYA_MIN_INTERVAL, BAHAYA_MAX_INTERVAL)

    def _is_malam(self) -> bool:
        return self.cycle_pos >= MALAM_MULAI

    def _clamp(self, value: float) -> float:
        return max(0.0, min(1.0, value))

    # -------------------------------------------------------------------------
    # GET ACTIVE SENSORS
    # Ini yang dikirim ke RAGP sebagai stimulus setiap saat
    # -------------------------------------------------------------------------
    def get_active_sensors(self) -> list:
        """
        Return list node ID yang sedang aktif/firing.
        RAGP akan menerima ini sebagai stimulus untuk compute_cd.
        """
        sensors = []

        # Sensor internal
        if self.lapar < THRESHOLD_LAPAR:
            sensors.append("103")   # SENSOR_LAPAR

        if self.lelah < THRESHOLD_LELAH:
            sensors.append("100")   # SENSOR_LELAH

        if self.health < THRESHOLD_SAKIT:
            sensors.append("104")   # SENSOR_SAKIT

        # Sensor eksternal - waktu
        if self._is_malam():
            sensors.append("101")   # MALAM
        else:
            sensors.append("105")   # SIANG

        # Sensor eksternal - lingkungan
        if self.semak_ada:
            sensors.append("102")   # SEMAK_ADA

        # Sensor bahaya
        if self.bahaya_aktif:
            sensors.append("1")     # SENSOR_BAHAYA

        return sensors

    # -------------------------------------------------------------------------
    # APPLY ACTION
    # -------------------------------------------------------------------------
    def apply_action(self, aksi_id: str) -> dict:
        """
        Terapkan aksi dari RAGP ke lingkungan.
        Return: dict berisi reward dan perubahan state.
        """
        if self.gugur:
            return {"reward": 0.0, "gugur": True, "pesan": "RAGP sudah gugur."}

        self.aksi_count += 1
        self.cycle_pos = (self.cycle_pos + 1) % CYCLE_PANJANG
        self.aksi_sejak_bahaya += 1

        health_sebelum = self.health
        pesan = []

        # 1. Terapkan cost aksi ke lapar dan lelah
        cost = ACTION_COST.get(aksi_id, {"lapar": 0.03, "lelah": 0.03})
        self.lapar = self._clamp(self.lapar - cost["lapar"])
        self.lelah = self._clamp(self.lelah - cost["lelah"])

        # 2. Terapkan recovery aksi
        recovery = ACTION_RECOVERY.get(aksi_id, {})
        if "lapar" in recovery:
            self.lapar = self._clamp(self.lapar + recovery["lapar"])
            pesan.append(f"lapar pulih +{recovery['lapar']:.2f}")
        if "lelah" in recovery:
            self.lelah = self._clamp(self.lelah + recovery["lelah"])
            pesan.append(f"lelah pulih +{recovery['lelah']:.2f}")
        if "health" in recovery:
            self.health = self._clamp(self.health + recovery["health"])
            pesan.append(f"health pulih +{recovery['health']:.2f}")

        # 3. Drain health jika lapar atau lelah = 0
        if self.lapar <= 0.0:
            self.health = self._clamp(self.health - DRAIN_LAPAR)
            pesan.append(f"kelaparan! health -{DRAIN_LAPAR:.2f}")

        if self.lelah <= 0.0:
            self.health = self._clamp(self.health - DRAIN_LELAH)
            pesan.append(f"kelelahan! health -{DRAIN_LELAH:.2f}")

        # 4. Proses bahaya
        if self.bahaya_aktif:
            if aksi_id in ["45", "88"]:
                # Berhasil menghindar
                damage = 0.0
                self.bahaya_aktif = False
                self.interval_bahaya = self._next_interval()
                self.aksi_sejak_bahaya = 0
                self.semak_ada = random.random() > 0.5  # update semak
                pesan.append("berhasil menghindar dari bahaya!")
            else:
                # Terkena bahaya
                damage = round(random.uniform(BAHAYA_DAMAGE_MIN, BAHAYA_DAMAGE_MAX), 2)
                self.health = self._clamp(self.health - damage)
                pesan.append(f"terkena bahaya! health -{damage:.2f}")

        # 5. Spawn bahaya baru jika waktunya
        if not self.bahaya_aktif and self.aksi_sejak_bahaya >= self.interval_bahaya:
            self.bahaya_aktif = True
            pesan.append("bahaya muncul!")

        # 6. Cek gugur
        if self.health <= 0.0:
            self.health = 0.0
            self.gugur = True
            pesan.append("RAGP GUGUR.")

        # 7. Hitung reward
        reward = round(self.health - health_sebelum, 4)

        result = {
            "reward":  reward,
            "gugur":   self.gugur,
            "health":  round(self.health, 3),
            "lapar":   round(self.lapar, 3),
            "lelah":   round(self.lelah, 3),
            "malam":   self._is_malam(),
            "bahaya":  self.bahaya_aktif,
            "sensors": self.get_active_sensors(),
            "pesan":   " | ".join(pesan) if pesan else "-",
        }

        self.log.append(result)
        return result

    # -------------------------------------------------------------------------
    # STATUS
    # -------------------------------------------------------------------------
    def status(self) -> str:
        """Tampilkan state saat ini dalam format yang mudah dibaca."""
        waktu = "MALAM" if self._is_malam() else "SIANG"
        bahaya = "ADA BAHAYA!" if self.bahaya_aktif else "aman"
        sensors = [translate(s) for s in self.get_active_sensors()]
        return (
            f"[Aksi #{self.aksi_count}] {waktu} | "
            f"Health={self.health:.2f} Lapar={self.lapar:.2f} Lelah={self.lelah:.2f} | "
            f"{bahaya} | Sensors={sensors}"
        )
