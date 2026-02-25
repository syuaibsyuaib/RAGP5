from environment import VirtualEnvironment, translate

print("="*65)
print(" TEST VIRTUAL ENVIRONMENT - RAGP5")
print("="*65)

env = VirtualEnvironment(seed=42)
print("\n[Init]", env.status())

# Skenario: RAGP melakukan serangkaian aksi
skenario = [
    ("108", "ISTIRAHAT"),
    ("108", "ISTIRAHAT"),
    ("106", "CARI_MAKAN"),
    ("106", "CARI_MAKAN"),
    ("107", "MAKAN"),
    ("45",  "LARI (saat bahaya muncul)"),
    ("108", "ISTIRAHAT"),
    ("106", "CARI_MAKAN"),
    ("107", "MAKAN"),
    ("88",  "BERSEMBUNYI"),
    ("109", "TIDUR"),
    ("109", "TIDUR"),
    ("106", "CARI_MAKAN"),
    ("107", "MAKAN"),
    ("45",  "LARI"),
    ("88",  "BERSEMBUNYI"),
    ("108", "ISTIRAHAT"),
    ("107", "MAKAN"),
    ("109", "TIDUR"),
    ("106", "CARI_MAKAN"),
]

print("\n--- Simulasi 20 Aksi ---\n")
for aksi_id, nama in skenario:
    result = env.apply_action(aksi_id)
    print(f"Aksi: {nama:30s} | reward={result['reward']:+.3f} | "
          f"H={result['health']:.2f} L={result['lapar']:.2f} Lt={result['lelah']:.2f} | "
          f"{result['pesan']}")
    if result["gugur"]:
        print("\n*** RAGP GUGUR ***")
        break

print("\n[Final]", env.status())
print(f"\nTotal aksi: {env.aksi_count}")
print(f"Status akhir: {'GUGUR' if env.gugur else 'HIDUP'}")
