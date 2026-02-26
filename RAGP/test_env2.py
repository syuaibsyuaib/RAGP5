from environment import VirtualEnvironment, translate

env = VirtualEnvironment(seed=42)
env.interval_bahaya = 2  # paksa bahaya muncul cepat
print("Test: RAGP selalu DIAM saat bahaya (tidak menghindar)\n")

for i in range(20):
    result = env.apply_action("12")  # selalu DIAM
    h  = result["health"]
    l  = result["lapar"]
    lt = result["lelah"]
    rw = result["reward"]
    ps = result["pesan"]
    print(f"Aksi {i+1:2d}: H={h:.2f} L={l:.2f} Lt={lt:.2f} | reward={rw:+.3f} | {ps}")
    if result["gugur"]:
        print("\nRAGP GUGUR")
        break
