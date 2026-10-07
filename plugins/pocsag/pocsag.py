#!/usr/bin/env python3
"""crabSDR-Decoder POCSAG: liest flachen FM-Ton (Betriebsart data, s16le mono, CRAB_RATE = 22050) von stdin, lässt
multimon-ng POCSAG 512/1200/2400 dekodieren und meldet jeden Ruf als JSON auf stdout (kind "page": ric, func, baud,
type alpha|numeric|tone, text). Übersicht pocsag.json (letzte KEEP Rufe) und Protokoll pocsag.jsonl im Datenordner.
Optionen: keep (300), baud (z. B. "1200" oder "512,1200,2400", Standard alle drei), charset (multimon-ng -C, z. B. DE).
"""
import json, os, re, subprocess, sys, threading, time
DATA = os.environ.get("CRAB_DATA", "."); RATE = os.environ.get("CRAB_RATE", "22050")
KEEP = int(os.environ.get("CRAB_OPT_KEEP", "300"))
BAUD = [b.strip() for b in os.environ.get("CRAB_OPT_BAUD", "512,1200,2400").split(",") if b.strip() in ("512", "1200", "2400")] or ["512", "1200", "2400"]
CHARSET = os.environ.get("CRAB_OPT_CHARSET", "")
OUT = os.path.join(DATA, "pocsag.json"); LOG = os.path.join(DATA, "pocsag.jsonl")
os.makedirs(DATA, exist_ok=True)
RX = re.compile(r"^POCSAG(512|1200|2400):\s+Address:\s*(\d+)\s+Function:\s*(\d)(?:\s+(Alpha|Numeric):\s*(.*))?\s*$")
lock = threading.Lock(); pages = []

def load():
    try:
        with open(OUT) as f: return json.load(f).get("pages", [])[:KEEP]
    except Exception: return []

def save():
    tmp = OUT + ".tmp"
    with open(tmp, "w") as f: json.dump({"ts": int(time.time()), "pages": pages[:KEEP]}, f, ensure_ascii=False)
    os.replace(tmp, OUT)

pages = load()
args = ["multimon-ng", "-t", "raw", "-q"] + sum([["-a", "POCSAG" + b] for b in BAUD], []) + ["-f", "alpha", "-u"]
if CHARSET: args += ["-C", CHARSET]
args.append("-")
mm = subprocess.Popen(args, stdin=sys.stdin, stdout=subprocess.PIPE, stderr=sys.stderr, text=True, errors="replace")
sys.stderr.write("pocsag: %s\n" % " ".join(args))
if int(RATE) != 22050: sys.stderr.write("pocsag: multimon-ng erwartet 22050 Hz, CRAB_RATE ist %s\n" % RATE)
for line in mm.stdout:
    line = line.rstrip("\n")
    m = RX.match(line)
    if not m:
        continue
    baud, ric, func, kind, text = m.groups()
    ev = {"kind": "page", "t": int(time.time()), "baud": int(baud), "ric": int(ric), "func": int(func),
          "type": (kind or "tone").lower(), "text": (text or "").strip()}
    with lock:
        pages.insert(0, ev); del pages[KEEP:]
        try:
            with open(LOG, "a") as lf: lf.write(json.dumps(ev, ensure_ascii=False) + "\n")
            save()
        except Exception as e: sys.stderr.write("pocsag: schreiben: %s\n" % e)
    sys.stdout.write(json.dumps(ev, ensure_ascii=False) + "\n"); sys.stdout.flush()
sys.exit(mm.wait())
