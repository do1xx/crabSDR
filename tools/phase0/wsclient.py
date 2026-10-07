#!/usr/bin/env python3
"""wsclient — Testclient für das crabSDR-Protokoll (Phase 0), ohne Fremdpakete.
Holt ein Gast-Token, öffnet /ws/<band>, stimmt ab und zählt den Datenstrom je Typ (Spektrum, Opus, PCM, JSON),
misst Abstände der Tonrahmen, Zeit bis zum ersten Ton, PCM-Lücken; Hörer 0 kann den Ton als WAV sichern.
   wsclient.py --band 70cm-o --freq 438925000 --sec 30 --wav out.wav       # 1 Hörer, PCM 48 kHz
   wsclient.py --band 70cm-o --freq 438925000 --sec 60 --n 20 --opus      # 20 Hörer, Opus, nur Statistik
Ausgabe: eine JSON-Zeile je Hörer und eine Zusammenfassung."""
import argparse, base64, json, os, socket, statistics, struct, sys, threading, time, urllib.request, wave

def http_json(url, data=None):
    body = json.dumps(data).encode() if data is not None else None
    req = urllib.request.Request(url, data=body, headers={"Content-Type": "application/json"},
                                 method="POST" if data is not None else "GET")
    with urllib.request.urlopen(req, timeout=10) as r:
        return json.loads(r.read())

def ws_connect(host, port, path):
    s = socket.create_connection((host, port), timeout=15)
    key = base64.b64encode(os.urandom(16)).decode()
    s.sendall((f"GET {path} HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n"
               f"Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: {key}\r\n\r\n").encode())
    resp = b""
    while b"\r\n\r\n" not in resp:
        c = s.recv(4096)
        if not c:
            raise RuntimeError("Verbindung beim Handshake geschlossen")
        resp += c
    head, rest = resp.split(b"\r\n\r\n", 1)
    status = head.split(b"\r\n")[0]
    if b" 101 " not in status:
        raise RuntimeError(status.decode(errors="replace") + " " + rest[:200].decode(errors="replace"))
    s.settimeout(10)
    return s, rest

def ws_send(s, payload, op=1):
    data = payload.encode() if isinstance(payload, str) else payload
    mask = os.urandom(4); n = len(data)
    if n < 126: hdr = bytes([0x80 | op, 0x80 | n])
    elif n < 65536: hdr = bytes([0x80 | op, 0x80 | 126]) + struct.pack(">H", n)
    else: hdr = bytes([0x80 | op, 0x80 | 127]) + struct.pack(">Q", n)
    s.sendall(hdr + mask + bytes(b ^ mask[i % 4] for i, b in enumerate(data)))

class Reader:
    def __init__(self, s, initial=b""):
        self.s = s; self.buf = bytearray(initial)
    def read_exact(self, n):
        while len(self.buf) < n:
            c = self.s.recv(262144)
            if not c:
                raise EOFError("Verbindung geschlossen")
            self.buf += c
        out = bytes(self.buf[:n]); del self.buf[:n]; return out
    def frame(self):
        h = self.read_exact(2); op = h[0] & 0x0F; ln = h[1] & 0x7F; masked = h[1] & 0x80
        if ln == 126: ln = struct.unpack(">H", self.read_exact(2))[0]
        elif ln == 127: ln = struct.unpack(">Q", self.read_exact(8))[0]
        mask = self.read_exact(4) if masked else None
        p = self.read_exact(ln)
        if mask: p = bytes(b ^ mask[i % 4] for i, b in enumerate(p))
        return op, p

KIND = {0x01: "spectrum", 0x81: "spectrum", 0x82: "opus", 0x02: "pcm", 0x03: "json", 0x84: "waterfall"}

def listener(i, a, token, results):
    st = {"i": i, "bytes": {"spectrum": 0, "opus": 0, "pcm": 0, "json": 0, "waterfall": 0, "other": 0},
          "frames": {"spectrum": 0, "opus": 0, "pcm": 0, "json": 0, "waterfall": 0, "other": 0}, "err": None, "sq_closed": 0, "level_last": None}
    gaps = []; pcm = bytearray(); t_first = None; t_last = None; last = None; pcm_samples = 0
    try:
        s, rest = ws_connect(a.host, a.port, f"/ws/{a.band}?token={token}")
        rd = Reader(s, rest)
        freq = a.freq + (i % a.spread) * a.spread_step if a.spread > 1 else a.freq
        ws_send(s, json.dumps({"type": "tune", "freq": freq, "mode": a.mode, "bandwidth": a.bw}))
        if a.opus:
            ws_send(s, json.dumps({"type": "set_codec", "audio": "opus"}))
        if a.agc:
            ws_send(s, json.dumps({"type": "set_agc", "mode": a.agc}))
        if a.squelch:
            ws_send(s, json.dumps({"type": "set_squelch", "mode": a.squelch, "db": a.squelch_db, "hang_ms": 500}))
        if a.wf is not None:
            ws_send(s, json.dumps({"type": "set_waterfall", "zoom": a.wf[0], "start_bin": a.wf[1]}))
        if a.full:
            ws_send(s, json.dumps({"type": "set_waterfall", "full": True}))
        if a.name:
            ws_send(s, json.dumps({"type": "set_name", "name": f"{a.name}{i}"}))
        t0 = time.monotonic()
        while time.monotonic() - t0 < a.sec:
            op, p = rd.frame(); now = time.monotonic()
            if op == 9: ws_send(s, p, 0xA); continue
            if op == 8: st["err"] = "Server hat geschlossen"; break
            if op == 1:
                st["bytes"]["json"] += len(p); st["frames"]["json"] += 1
                st.setdefault("json_first", p[:160].decode(errors="replace")); continue
            if op != 2 or not p: continue
            kind = KIND.get(p[0], "other")
            st["bytes"][kind] += len(p); st["frames"][kind] += 1
            if kind == "json":
                try:
                    j = json.loads(p[1:])
                    if j.get("type") == "level":
                        st["level_last"] = j
                        if not j.get("sq", True): st["sq_closed"] += 1
                except Exception: pass
            if kind in ("opus", "pcm"):
                if t_first is None: t_first = now; st["first_audio_ms"] = round((now - t0) * 1000)
                if last is not None: gaps.append(now - last)
                last = now; t_last = now
                if kind == "pcm":
                    pcm_samples += (len(p) - 1) // 2
                    if a.wav and i == 0: pcm += p[1:]
        s.close()
    except Exception as e:
        st["err"] = repr(e)
    st["kbit_s"] = {k: round(v * 8 / a.sec / 1000, 1) for k, v in st["bytes"].items()}
    st["kbit_s_total"] = round(sum(st["bytes"].values()) * 8 / a.sec / 1000, 1)
    if gaps:
        g = sorted(gaps); n = len(g)
        st["audio_gap_ms"] = {"mean": round(statistics.mean(g) * 1000, 1), "p50": round(g[n // 2] * 1000, 1),
                              "p99": round(g[int(n * 0.99)] * 1000, 1), "max": round(g[-1] * 1000, 1)}
    if pcm_samples and t_first is not None and t_last and t_last > t_first:
        expected = (t_last - t_first) * 48000
        st["pcm_samples"] = pcm_samples
        st["pcm_missing_pct"] = round(max(0.0, 1 - pcm_samples / expected) * 100, 2)
    if a.wav and i == 0 and pcm:
        with wave.open(a.wav, "wb") as w:
            w.setnchannels(1); w.setsampwidth(2); w.setframerate(48000); w.writeframes(bytes(pcm))
        st["wav"] = a.wav
    results[i] = st

ap = argparse.ArgumentParser()
ap.add_argument("--host", default="127.0.0.1"); ap.add_argument("--port", type=int, default=8080)
ap.add_argument("--band", required=True); ap.add_argument("--freq", type=int, required=True)
ap.add_argument("--mode", default="fm"); ap.add_argument("--bw", type=int, default=12500)
ap.add_argument("--sec", type=float, default=30); ap.add_argument("--n", type=int, default=1)
ap.add_argument("--opus", action="store_true"); ap.add_argument("--wav", default=None)
ap.add_argument("--agc", default=None, help="off|fast|medium|slow (Server-Standard: medium)")
ap.add_argument("--squelch", default=None, help="off|auto|manual"); ap.add_argument("--squelch-db", type=float, default=-60)
ap.add_argument("--wf", type=int, nargs=2, default=None, metavar=("ZOOM", "START_BIN"), help="Wasserfall-Zeilen abonnieren")
ap.add_argument("--full", action="store_true", help="altes Vollspektrum (0x81) anfordern"); ap.add_argument("--name", default=None)
ap.add_argument("--stagger", type=float, default=0.05, help="Sekunden zwischen den Verbindungsaufbauten")
ap.add_argument("--spread", type=int, default=1, help="Hörer auf N verschiedene Frequenzen verteilen (freq + k*spread_step)")
ap.add_argument("--spread-step", type=int, default=12500)
a = ap.parse_args()
base = f"http://{a.host}:{a.port}"
token = http_json(base + "/api/auth/guest", {})["token"]
results = {}
th = []
for i in range(a.n):
    t = threading.Thread(target=listener, args=(i, a, token, results)); t.start(); th.append(t)
    time.sleep(a.stagger)
for t in th: t.join()
for i in sorted(results): print(json.dumps(results[i], ensure_ascii=False))
ok = [r for r in results.values() if not r["err"]]
if ok:
    summ = {"n": a.n, "ok": len(ok), "kbit_s_per_client_mean": round(statistics.mean(r["kbit_s_total"] for r in ok), 1),
            "kbit_s_sum": round(sum(r["kbit_s_total"] for r in ok), 1),
            "audio_gap_max_ms": max((r.get("audio_gap_ms", {}).get("max", 0) for r in ok), default=0),
            "audio_gap_p99_ms_max": max((r.get("audio_gap_ms", {}).get("p99", 0) for r in ok), default=0),
            "first_audio_ms_max": max((r.get("first_audio_ms", 0) for r in ok), default=0),
            "pcm_missing_pct_max": max((r.get("pcm_missing_pct", 0) for r in ok), default=0)}
    print("SUMMARY " + json.dumps(summ))
