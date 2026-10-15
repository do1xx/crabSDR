#!/usr/bin/env python3
"""crabSDR-Decoder POCSAG: liest flachen FM-Ton (Betriebsart data, s16le mono, CRAB_RATE = 22050) von stdin, lässt
multimon-ng POCSAG 512/1200/2400 dekodieren und meldet jeden Ruf als JSON auf stdout (kind "page": ric, func, baud,
type alpha|numeric|tone, text). Übersicht pocsag.json (letzte KEEP Rufe) und Protokoll pocsag.jsonl im Datenordner.
Optionen: keep (300), baud (z. B. "1200" oder "512,1200,2400", Standard alle drei), charset (multimon-ng -C, z. B. DE).
DAPNET-Rubriken (Skyper-Format: Zeichen 1 = Rubrik + 0x1F, Zeichen 2 = Nummer + 0x1F, Rest um eins verschoben, Umlaute nach
ISO 646 DE) werden lesbar gemacht: text = "Rubrik <r>/<n>: …" – auf RIC 4520 immer, auf anderen RICs (Rubrik an Abonnenten),
wenn der Rest verschoben aussieht (mehr „!“ als Leerzeichen, mindestens zwei). Steuerzeichen-Marken von multimon-ng (<NUL> …) fallen weg.
"""
import json, os, re, subprocess, sys, threading, time
DATA = os.environ.get("CRAB_DATA", "."); RATE = os.environ.get("CRAB_RATE", "22050")
KEEP = int(os.environ.get("CRAB_OPT_KEEP", "300"))
BAUD = [b.strip() for b in os.environ.get("CRAB_OPT_BAUD", "512,1200,2400").split(",") if b.strip() in ("512", "1200", "2400")] or ["512", "1200", "2400"]
CHARSET = os.environ.get("CRAB_OPT_CHARSET", "")
OUT = os.path.join(DATA, "pocsag.json"); LOG = os.path.join(DATA, "pocsag.jsonl")
os.makedirs(DATA, exist_ok=True)
RX = re.compile(r"^POCSAG(512|1200|2400):\s+Address:\s*(\d+)\s+Function:\s*(\d)(?:\s+(Alpha|Numeric):\s*(.*))?\s*$")
CTRL = re.compile(r"<(NUL|SOH|STX|ETX|EOT|ENQ|ACK|BEL|BS|HT|LF|VT|FF|CR|SO|SI|DLE|DC[1-4]|NAK|SYN|ETB|CAN|EM|SUB|ESC|FS|GS|RS|US|DEL)>")
RUBRIC_RIC = 4520   # DAPNET/Skyper: Rubriken (DX-Cluster, Pegel, Nachrichten …)
DE = str.maketrans("[\\]{|}~", "ÄÖÜäöüß")   # ISO 646 DE, wie multimon-ng -C DE

def looks_shifted(body):
    """Verschobener Text: Leerzeichen werden zu „!“ – mindestens zwei „!“ und mehr „!“ als Leerzeichen (Klartext „Test 73!“ nicht)."""
    return body.count("!") >= 2 and body.count("!") > body.count(" ")

def clean(ric, text):
    """Steuerzeichen-Marken weg; DAPNET-Rubrik (Skyper-Format) entschlüsseln: Rubrik/Nummer aus den ersten zwei Zeichen, Rest −1."""
    text = CTRL.sub("", text)
    if len(text) >= 2 and all(0x20 <= ord(c) < 0x7f for c in text[:2]) and (ric == RUBRIC_RIC or looks_shifted(text[2:])):
        rubric, nr = ord(text[0]) - 0x1f, ord(text[1]) - 0x1f
        body = "".join(chr(ord(c) - 1) if 0x21 <= ord(c) < 0x80 else c for c in text[2:]).translate(DE)
        return "Rubrik %d/%d: %s" % (rubric, nr, body.strip())
    return text.strip()
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
          "type": (kind or "tone").lower(), "text": clean(int(ric), text or "")}
    with lock:
        pages.insert(0, ev); del pages[KEEP:]
        try:
            with open(LOG, "a") as lf: lf.write(json.dumps(ev, ensure_ascii=False) + "\n")
            save()
        except Exception as e: sys.stderr.write("pocsag: schreiben: %s\n" % e)
    sys.stdout.write(json.dumps(ev, ensure_ascii=False) + "\n"); sys.stdout.flush()
sys.exit(mm.wait())
