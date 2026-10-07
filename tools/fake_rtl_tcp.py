# Nachgebauter rtl_tcp für Tests ohne Hardware: Kopf "RTL0", dann Rauschen mit einem Träger 300 kHz über der Mitte;
# empfängt die 5-Byte-Befehle (Frequenz, Abtastrate, Gain) und schreibt sie auf stdout. Aufruf: python3 tools/fake_rtl_tcp.py 6999
# Band dazu: driver = "rtl_tcp", host = "127.0.0.1", port = 6999
import socket, struct, sys, threading, time, math, random
port = int(sys.argv[1]) if len(sys.argv) > 1 else 6999
state = {"freq": 145_000_000, "rate": 2_048_000}
def client(c):
    c.sendall(b"RTL0" + struct.pack(">II", 5, 29))   # Tuner R820T, 29 Gain-Stufen
    def reader():
        while True:
            b = c.recv(5)
            if len(b) < 5: return
            cmd, val = b[0], struct.unpack(">I", b[1:])[0]
            if cmd == 1: state["freq"] = val; print(f"[fake] SET_FREQ {val} Hz", flush=True)
            elif cmd == 2: state["rate"] = val; print(f"[fake] SET_RATE {val}", flush=True)
            else: print(f"[fake] cmd {cmd} = {val}", flush=True)
    threading.Thread(target=reader, daemon=True).start()
    n = 0; chunk = 16384
    try:
        while True:
            # Träger 300 kHz über der Mitte + Rauschen, u8 I/Q
            buf = bytearray(chunk * 2)
            for i in range(chunk):
                ph = 2 * math.pi * 300_000 * (n + i) / state["rate"]
                buf[2*i] = max(0, min(255, int(127.5 + 40 * math.cos(ph) + random.gauss(0, 8))))
                buf[2*i+1] = max(0, min(255, int(127.5 + 40 * math.sin(ph) + random.gauss(0, 8))))
            n += chunk
            c.sendall(buf)
            time.sleep(chunk / state["rate"])
    except Exception as e:
        print("[fake] client weg:", e, flush=True)
s = socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1); s.bind(("127.0.0.1", port)); s.listen(2)
print(f"[fake] rtl_tcp auf {port}", flush=True)
while True:
    c, _ = s.accept(); threading.Thread(target=client, args=(c,), daemon=True).start()
