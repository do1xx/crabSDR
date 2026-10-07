#!/usr/bin/env python3
"""crabSDR-Decoder FT8 : liest 12-kHz-USB-Ton (s16le mono) von stdin, schneidet UTC-synchron
15-s-Abschnitte, decodiert sie mit jt9 (WSJT-X), meldet jede Zeile als JSON auf stdout (kind "decode") und schreibt
eine Übersicht (ft8.json: Decodes + Stationen) und ein Protokoll (ft8.jsonl).
Optionen: json (Pfad der Übersicht, Standard $CRAB_DATA/ft8.json), log (Standard $CRAB_DATA/ft8.jsonl), keep (300),
days (Stationen der letzten n Tage, Standard 90),
depth (Suchtiefe von jt9 1–3, Standard 3; 2 spart auf einem Pi 4 deutlich Rechenzeit, verliert nur die schwächsten Signale).
"""
import json, os, re, subprocess, sys, threading, time, wave
DATA = os.environ.get("CRAB_DATA", ".")
RATE = int(os.environ.get("CRAB_RATE", "12000"))
SEG = 15
WORK = os.path.join(DATA, "work")
LOG = os.environ.get("CRAB_OPT_LOG") or os.path.join(DATA, "ft8.jsonl")
OUT = os.environ.get("CRAB_OPT_JSON") or os.path.join(DATA, "ft8.json")
KEEP = int(os.environ.get("CRAB_OPT_KEEP", "300"))
DEPTH = os.environ.get("CRAB_OPT_DEPTH", "3")
RX = re.compile(r"^(\d{6})\s+(-?\d+)\s+(-?\d+\.\d)\s+(\d+)\s+~\s+(.*?)\s*$")
GRID = re.compile(r"\b(?!RR73\b)([A-R]{2}[0-9]{2})\b")   # RR73 = FT8-Schlussgruß, kein Locator
CALL = re.compile(r"\b([A-Z0-9]{1,3}[0-9][A-Z0-9]{0,3}[A-Z](?:/[A-Z0-9]+)?)\b")

os.makedirs(WORK, exist_ok=True); os.makedirs(os.path.dirname(LOG), exist_ok=True); os.makedirs(os.path.dirname(OUT), exist_ok=True)
DAYS = int(os.environ.get("CRAB_OPT_DAYS", "90"))
lock = threading.Lock()
decodes = []          # letzte KEEP Decodes (Liste auf der Seite)
stations = {}         # alle Sender der letzten DAYS Tage (Karte, Reichweite)


def add_station(x):
    if not x.get("call"):
        return
    s = stations.setdefault(x["call"], {"call": x["call"], "n": 0, "last": 0, "snr": -99})
    s["n"] += 1; s["last"] = max(s["last"], x["t"]); s["snr"] = max(s["snr"], x["snr"])
    if x.get("grid") and x["grid"] != "RR73":     # ältere Protokollzeilen können RR73 noch als Locator tragen
        s["grid"] = x["grid"]; s["lat"], s["lon"] = x["lat"], x["lon"]


# Protokoll laden; Einträge älter als DAYS fallen beim Start heraus (Datei wird dann gekürzt)
try:
    old, cut = [], time.time() - DAYS * 86400
    with open(LOG) as f:
        for line in f:
            try: old.append(json.loads(line))
            except Exception: pass
    keep = [x for x in old if x.get("t", 0) >= cut]
    if len(keep) < len(old):
        with open(LOG + ".tmp", "w") as f:
            for x in keep: f.write(json.dumps(x, ensure_ascii=False) + "\n")
        os.replace(LOG + ".tmp", LOG)
    for x in keep: add_station(x)
    decodes = keep[-KEEP:]
except Exception:
    pass


def grid2ll(g):
    g = g.upper()
    lon = (ord(g[0]) - 65) * 20 - 180 + int(g[2]) * 2 + 1
    lat = (ord(g[1]) - 65) * 10 - 90 + int(g[3]) + 0.5
    return lat, lon


def write_out():
    cut = time.time() - DAYS * 86400
    with lock:
        d = list(decodes)
        st = [dict(s) for s in stations.values() if s["last"] >= cut]
    out = {"ts": int(time.time()), "days": DAYS, "decodes": d[::-1], "stations": sorted(st, key=lambda s: -s["last"])}
    tmp = OUT + ".tmp"
    json.dump(out, open(tmp, "w"), ensure_ascii=False); os.replace(tmp, OUT)


def decode(path, t0):
    try:
        r = subprocess.run(["jt9", "--ft8", "-d", DEPTH, path], cwd=WORK, capture_output=True, text=True, timeout=40)
        new = []
        for line in r.stdout.splitlines():
            m = RX.match(line)
            if not m:
                continue
            hhmmss, snr, dt, f, msg = m.groups()
            toks = msg.split()
            call = None
            # Sender = zweites Rufzeichen bei "CQ X GRID", sonst das zweite Token (Antwortende Station)
            if toks and toks[0] in ("CQ", "QRZ", "DE") and len(toks) >= 2:
                cands = [t for t in toks[1:] if CALL.fullmatch(t)]
                call = cands[0] if cands else None
            elif len(toks) >= 2 and CALL.fullmatch(toks[1]):
                call = toks[1]
            g = GRID.search(msg); grid = g.group(1) if g else None
            lat, lon = grid2ll(grid) if grid else (None, None)
            new.append({"t": t0, "snr": int(snr), "dt": float(dt), "f": int(f), "msg": msg, "call": call, "grid": grid, "lat": lat, "lon": lon})
        if new:
            with lock, open(LOG, "a") as lf:
                for x in new:
                    decodes.append(x); add_station(x); lf.write(json.dumps(x, ensure_ascii=False) + "\n")
                    sys.stdout.write(json.dumps(dict(x, kind="decode"), ensure_ascii=False) + "\n")
                sys.stdout.flush()
                del decodes[:-KEEP]
            sys.stderr.write(f"{time.strftime('%H:%M:%S')} {len(new)} FT8-Decodes\n")
        write_out()
    except Exception as e:
        sys.stderr.write(f"jt9: {e}\n")
    finally:
        try: os.remove(path)
        except Exception: pass


def main():
    write_out()
    src = sys.stdin.buffer
    bps = 2
    # bis zur nächsten 15-s-Grenze verwerfen, dann segmentweise sammeln
    def now(): return time.time()
    t = now(); wait = SEG - (t % SEG)
    src.read(int(wait * RATE) * bps)
    while True:
        # Gleichlauf halten: fehlte Ton (Neustart, Aussetzer des Sticks), liegt der Abschnitt nicht mehr auf der
        # 15-s-Grenze -> bis zur nächsten verwerfen (jt9 verträgt nur ±2,5 s Versatz)
        lag = now() % SEG
        if 1.0 < lag < SEG - 1.0:
            src.read(int((SEG - lag) * RATE) * bps)
        t0 = int(round(now() / SEG) * SEG)
        buf = bytearray()
        need = SEG * RATE * bps
        while len(buf) < need:
            chunk = src.read(need - len(buf))
            if not chunk:
                return
            buf += chunk
        path = os.path.join(WORK, time.strftime("%y%m%d_%H%M%S", time.gmtime(t0)) + ".wav")
        w = wave.open(path, "wb"); w.setnchannels(1); w.setsampwidth(2); w.setframerate(RATE); w.writeframes(bytes(buf)); w.close()
        threading.Thread(target=decode, args=(path, t0), daemon=True).start()


if __name__ == "__main__":
    main()
