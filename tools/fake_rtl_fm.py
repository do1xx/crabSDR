# Nachgebauter rtl_tcp mit FM-moduliertem Ton: legt eine WAV-Datei (Hub 3 kHz) auf einen Träger 300 kHz über der Mitte,
# zwischen den Durchläufen 6 s nur Rauschen. Für Decoder-Tests (SSTV, APRS …) ohne Hardware:
#   python3 tools/fake_rtl_fm.py 6999 bild.wav [Trägeramplitude, Standard 40; 8 ≈ 18 dB, 4 ≈ 12 dB S/N im Kanal]
# Band dazu: driver = "rtl_tcp", host = "127.0.0.1", port = 6999 – der Decoder gehört auf Mitte + 300 kHz. Braucht numpy.
import socket, struct, sys, threading, time, wave, numpy as np
port = int(sys.argv[1]); wavp = sys.argv[2]
w = wave.open(wavp, "rb"); afs = w.getframerate(); a = np.frombuffer(w.readframes(w.getnframes()), dtype=np.int16).astype(np.float32) / 32768
if w.getnchannels() == 2: a = a[::2]
state = {"freq": 145_000_000, "rate": 2_048_000}
OFF, DEV, GAP = 300_000.0, 3000.0, 6.0
AMP = float(sys.argv[3]) if len(sys.argv) > 3 else 40.0
def client(c):
    c.sendall(b"RTL0" + struct.pack(">II", 5, 29))
    def reader():
        while True:
            b = c.recv(5)
            if len(b) < 5: return
            cmd, val = b[0], struct.unpack(">I", b[1:])[0]
            if cmd == 1: state["freq"] = val
            elif cmd == 2: state["rate"] = val; print(f"[fake] SET_RATE {val}", flush=True)
    threading.Thread(target=reader, daemon=True).start()
    time.sleep(0.5)
    rate = state["rate"]; blk = rate // 10; phase = 0.0; t_audio = 0.0; n = 0; t0 = time.time()
    total = len(a) / afs + GAP
    rng = np.random.default_rng(1)
    print(f"[fake] rate {rate}, WAV {len(a)/afs:.1f} s, Pause {GAP} s", flush=True)
    try:
        while True:
            tt = (np.arange(blk) + n) / rate
            pos = tt % total
            aud = np.interp(pos * afs, np.arange(len(a)), a, left=0.0, right=0.0)
            aud[pos >= len(a) / afs] = 0.0
            on = pos < len(a) / afs
            inst = OFF + DEV * aud
            ph = phase + 2 * np.pi * np.cumsum(inst) / rate
            phase = ph[-1] % (2 * np.pi)
            amp = np.where(on, AMP, 0.0)
            i = 127.5 + amp * np.cos(ph) + rng.normal(0, 8, blk)
            q = 127.5 + amp * np.sin(ph) + rng.normal(0, 8, blk)
            buf = np.empty(blk * 2, dtype=np.uint8); buf[0::2] = np.clip(i, 0, 255); buf[1::2] = np.clip(q, 0, 255)
            c.sendall(buf.tobytes()); n += blk
            dt = t0 + n / rate - time.time()
            if dt > 0: time.sleep(dt)
    except Exception as e:
        print("[fake] client weg:", e, flush=True)
s = socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1); s.bind(("127.0.0.1", port)); s.listen(2)
print(f"[fake] rtl_tcp auf {port}", flush=True)
while True:
    c, _ = s.accept(); threading.Thread(target=client, args=(c,), daemon=True).start()
