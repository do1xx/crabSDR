#!/usr/bin/env python3
"""iqplay — rtl_tcp-Nachbildung: spielt eine .cu8-Datei in Echtzeit (Standard 2,048 MS/s) an beliebig viele Abnehmer,
Endlosschleife (am Dateiende ein Sprung). Befehle der Abnehmer werden gelesen und verworfen.
Deterministische Quelle für Messungen und Tests ohne Hardware.
   iqplay.py testdata/70cm-o.cu8 --port 7903"""
import argparse, socket, threading, time

ap = argparse.ArgumentParser()
ap.add_argument("file"); ap.add_argument("--port", type=int, default=7901)
ap.add_argument("--rate", type=int, default=2048000); ap.add_argument("--chunk", type=int, default=65536)
ap.add_argument("--wallclock", action="store_true", help="Datei an der Uhr ausrichten: Sekunde 0 = volle Minute (für FT8-Tests mit 60-s-Dateien)")
a = ap.parse_args()
data = open(a.file, "rb").read()
HDR = b"RTL0" + (5).to_bytes(4, "big") + (29).to_bytes(4, "big")   # Tuner R820T, 29 Gain-Stufen
bps = a.rate * 2

def drain(c):
    try:
        while c.recv(4096):
            pass
    except Exception:
        pass

def serve(c, addr):
    c.sendall(HDR)
    threading.Thread(target=drain, args=(c,), daemon=True).start()
    t0 = time.monotonic(); sent = 0; pos = 0
    if a.wallclock:
        pos = int((time.time() % (len(data) / bps)) * a.rate) * 2
    try:
        while True:
            end = min(pos + a.chunk, len(data))
            c.sendall(data[pos:end]); sent += end - pos; pos = end
            if pos >= len(data):
                pos = 0
            d = t0 + sent / bps - time.monotonic()
            if d > 0:
                time.sleep(d)
    except Exception:
        pass
    print(f"Abnehmer {addr} weg nach {sent/bps:.1f} s", flush=True)
    c.close()

srv = socket.socket(); srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
srv.bind(("127.0.0.1", a.port)); srv.listen(50)
print(f"iqplay {a.file} ({len(data)/bps:.1f} s) auf 127.0.0.1:{a.port}", flush=True)
while True:
    c, addr = srv.accept()
    c.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
    print(f"Abnehmer {addr}", flush=True)
    threading.Thread(target=serve, args=(c, addr), daemon=True).start()
