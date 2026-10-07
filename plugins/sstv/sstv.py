#!/usr/bin/env python3
"""crabSDR-Decoder SSTV : liest FM-Ton (s16le mono, CRAB_RATE) von stdin, erkennt den
VIS-Kopf, nimmt die zum Modus passende Dauer auf, decodiert mit sstvdec.py (eigener numpy-Decoder: Martin, Scottie,
Robot 36/72, PD 50–290) und legt das Bild samt Index ab. Meldet auf stdout: kind "vis" (Bild beginnt), "image".
Ausgabe: <out>/sstv/JJJJ-MM/JJJJMMTT_HHMMSS_<kHz>_<Modus>.png und <out>/sstv.json (neueste zuerst);
<out> = Option out, Standard $CRAB_DATA."""
import argparse, json, os, sys, threading, time
import numpy as np
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import sstvdec

ap = argparse.ArgumentParser()
E = os.environ.get
ap.add_argument("--freq", type=float, default=float(E("CRAB_FREQ", "0")), help="Kanalfrequenz (Hz), nur für die Beschriftung")
ap.add_argument("--rate", type=int, default=int(E("CRAB_RATE", "24000")))
ap.add_argument("--out", default=E("CRAB_OPT_OUT") or E("CRAB_DATA", "."))
ap.add_argument("--keep", type=int, default=int(E("CRAB_OPT_KEEP", "500")), help="Einträge im Index")
a = ap.parse_args()
FS = a.rate
IMGDIR = os.path.join(a.out, "sstv"); INDEX = os.path.join(a.out, "sstv.json")
os.makedirs(IMGDIR, exist_ok=True)
lock = threading.Lock()
jobs = []


def log(msg):
    sys.stderr.write(time.strftime("%H:%M:%S ") + msg + "\n"); sys.stderr.flush()


def emit(ev):
    with lock:
        sys.stdout.write(json.dumps(ev, ensure_ascii=False) + "\n"); sys.stdout.flush()


def add_index(entry):
    with lock:
        try: idx = json.load(open(INDEX))
        except Exception: idx = {"images": []}
        idx["images"].insert(0, entry); idx["images"] = idx["images"][:a.keep]; idx["ts"] = int(time.time())
        tmp = INDEX + ".tmp"; json.dump(idx, open(tmp, "w"), ensure_ascii=False); os.replace(tmp, INDEX)


def decode_job(samples, vis, t_start):
    try:
        r = sstvdec.decode(samples, FS, vis=vis)
    except Exception as e:
        log(f"Decoder-Fehler: {e}"); return
    if not r:
        log(f"VIS {vis} erkannt, aber kein brauchbares Bild"); return
    name, im, info = r
    sub = time.strftime("%Y-%m", time.gmtime(t_start)); os.makedirs(os.path.join(IMGDIR, sub), exist_ok=True)
    fn = time.strftime("%Y%m%d_%H%M%S", time.gmtime(t_start)) + f"_{int(a.freq/1000)}_{name.replace(' ', '')}.png"
    path = os.path.join(IMGDIR, sub, fn); im.save(path, optimize=True)
    entry = {"t": int(t_start), "freq": int(a.freq), "mode": name, "file": f"sstv/{sub}/{fn}", "w": im.width, "h": im.height,
             "lines": info["lines"], "of": info["of"]}
    add_index(entry); log(f"Bild gespeichert: {fn} ({info['lines']}/{info['of']} Zeilen)")
    emit(dict(entry, kind="image"))


def main():
    src = sys.stdin.buffer
    chunk = int(FS * 0.25) * 2                       # 250 ms
    ring = np.zeros(0, dtype=np.int16)
    RING = int(FS * 2.0)
    recording = None                                # dict(samples list, need, vis, t)
    add_index_dummy = not os.path.exists(INDEX)
    if add_index_dummy:
        json.dump({"images": [], "ts": int(time.time())}, open(INDEX, "w"))
    log(f"SSTV-Lauscher auf {a.freq/1e6:.3f} MHz, {FS} Hz")
    while True:
        raw = src.read(chunk)
        if not raw: break
        x = np.frombuffer(raw, dtype=np.int16)
        if recording is not None:
            recording["buf"].append(x); recording["have"] += len(x)
            if recording["have"] >= recording["need"]:
                samples = np.concatenate(recording["buf"]).astype(np.float32) / 32768
                th = threading.Thread(target=decode_job, args=(samples, recording["vis"], recording["t"]), daemon=True); th.start(); jobs.append(th)
                recording = None; ring = np.zeros(0, dtype=np.int16)
            continue
        ring = np.concatenate((ring, x))[-RING:]
        if len(ring) < int(FS * 1.2): continue
        r = sstvdec.find_vis(ring.astype(np.float32) / 32768, FS)
        if r:
            vis, end = r
            name = sstvdec.MODES.get(vis, {}).get("name", f"VIS {vis}")
            dur = sstvdec.duration_s(vis)
            # Aufnahme beginnt 1,0 s vor dem VIS-Ende (Kopf muss mit rein), Bild dauert dur Sekunden
            start = max(0, end - int(FS * 1.0))
            pre = ring[start:]
            recording = {"buf": [pre], "have": len(pre), "need": len(pre) + int(dur * FS), "vis": vis, "t": time.time() - (len(ring) - end) / FS}
            log(f"VIS {vis} = {name}, nehme {dur:.0f} s auf")
            emit({"kind": "vis", "t": int(time.time()), "freq": int(a.freq), "mode": name, "seconds": round(dur)})
    if recording is not None and recording["have"] > FS * 5:      # Rest bei Stream-Ende noch decodieren
        samples = np.concatenate(recording["buf"]).astype(np.float32) / 32768
        th = threading.Thread(target=decode_job, args=(samples, recording["vis"], recording["t"]), daemon=True); th.start(); jobs.append(th)
    for th in jobs: th.join(120)
    log("Eingabe zu Ende")


if __name__ == "__main__":
    main()
