#!/usr/bin/env python3
"""crabSDR-Decoder APRS: direwolf dekodiert den FM-Ton (s16le mono, CRAB_RATE) von stdin. Die Pakete schreibt direwolf
in seine Tages-CSV (-l Verzeichnis); dieses Skript liest sie mit und meldet jede neue Zeile als JSON auf stdout.
Optionen (config.toml [decoders.options]): logdir (Standard: $CRAB_DATA/log), mycall (Standard N0CALL, nur Empfang).
iGate (Empfangenes an APRS-IS / aprs.fi weitergeben, nie per Funk senden): igate = "on" (Rufzeichen = igate_call oder
mycall; der APRS-IS-Passcode wird nach dem üblichen öffentlichen Verfahren aus dem Rufzeichen berechnet, optional
igate_passfile), igate_server (Standard euro.aprs2.net). Bake: lat, lon (Dezimalgrad, Standard: Standort aus [station]),
symbol (Standard R&), comment,
every (Minuten, Standard 30).
Für die Seite „Digital" schreibt es alle 60 s aprs.json und relais.json in den Datenordner (aprssummary.py).
"""
import csv, json, os, subprocess, sys, threading, time

RATE = os.environ.get("CRAB_RATE", "48000")
DATA = os.environ.get("CRAB_DATA", ".")
LOGDIR = os.environ.get("CRAB_OPT_LOGDIR") or os.path.join(DATA, "log")
MYCALL = os.environ.get("CRAB_OPT_MYCALL", "N0CALL")
os.makedirs(LOGDIR, exist_ok=True)
O = lambda k, d="": os.environ.get("CRAB_OPT_" + k.upper(), d)
conf = os.path.join(DATA, "direwolf.conf")
lines = ["ADEVICE stdin null", "ARATE %s" % RATE, "MYCALL %s" % MYCALL, "MODEM 1200", "AGWPORT 0", "KISSPORT 0"]
# Ton von stdin (ADEVICE statt „-“ als Argument: das versteht direwolf 1.6 nicht); nur Empfang, keine Netzwerk-Ports
def aprs_passcode(call):
    """APRS-IS-Passcode (öffentliches Verfahren, wie in aprsd/Xastir): Hash über das Rufzeichen ohne SSID."""
    c = call.upper().split("-")[0]; h = 0x73E2
    for i in range(0, len(c), 2):
        h ^= ord(c[i]) << 8
        if i + 1 < len(c): h ^= ord(c[i + 1])
    return str(h & 0x7FFF)


call = O("igate_call", MYCALL)
passfile, igate_on = O("igate_passfile"), O("igate").lower() in ("1", "on", "ja", "true", "yes") or bool(O("igate_call"))
if (igate_on or passfile) and call.upper().split("-")[0] not in ("N0CALL", "NOCALL"):
    pc = ""
    if passfile:
        try: pc = open(passfile).read().strip()
        except OSError as e: sys.stderr.write("aprs: iGate-Passcode-Datei nicht lesbar (%s), berechne ihn\n" % e)
    pc = pc or aprs_passcode(call)
    if pc:
        lines += ["IGSERVER %s" % O("igate_server", "euro.aprs2.net"), "IGLOGIN %s %s" % (call, pc)]
        # kein IGTXVIA: nichts vom Internet per Funk senden (ohnehin kein Sender angeschlossen)
        lat, lon = O("lat") or os.environ.get("CRAB_STATION_LAT", ""), O("lon") or os.environ.get("CRAB_STATION_LON", "")   # Standard: [station]
        if lat and lon:
            sym = O("symbol", "R&")
            symarg = 'symbol="%s"' % sym if sym[0] in "/\\" else 'symbol="\\%s" overlay=%s' % (sym[1], sym[0])
            lines.append('PBEACON sendto=IG delay=0:30 every=%s lat=%s long=%s %s comment="%s"' % (O("every", "30"), lat, lon, symarg, O("comment", "crabSDR iGate").replace('"', "'")))
        sys.stderr.write("aprs: iGate an %s als %s\n" % (O("igate_server", "euro.aprs2.net"), call))
with open(conf, "w") as f:
    f.write("\n".join(lines) + "\n")
os.chmod(conf, 0o600)                         # enthält ggf. den Passcode


def num(v):
    try:
        return float(v) if v not in (None, "") else None
    except ValueError:
        return None


def emit(r):
    src, heard = (r.get("source") or "").strip(), (r.get("heard") or "").strip()
    lat, lon = num(r.get("latitude")), num(r.get("longitude"))
    if lat is None or lon is None:
        lat = lon = None                      # kaputt formatierte Meldung: keine halbe Position melden
    ev = {"kind": "position" if lat is not None else "packet", "t": int(num(r.get("utime")) or time.time()),
          "src": src, "heard": heard, "direct": heard == src, "digi": "" if heard == src else heard.rstrip("?"),
          "name": r.get("name") or "", "sym": r.get("symbol") or "", "lat": lat, "lon": lon,
          "alt": num(r.get("altitude")), "speed": num(r.get("speed")), "course": num(r.get("course")),
          "level": r.get("level") or "", "dti": r.get("dti") or "", "status": r.get("status") or "",
          "comment": r.get("comment") or ""}
    sys.stdout.write(json.dumps(ev, ensure_ascii=False) + "\n"); sys.stdout.flush()


def clean(raw):
    """direwolf schreibt das Datentyp-Zeichen eines Pakets roh in die CSV. Ist es ein Wagenrücklauf oder NUL, bricht
    csv.reader ab („new-line character seen in unquoted field“) – so starb der Lese-Thread still, direwolf lief weiter und
    es kamen keine Pakete mehr (DF0HHH/DM0NDR, Oktober 2026)."""
    return raw.replace(b"\r", b"").replace(b"\x00", b"").decode("utf-8", "replace")


def tail():
    """Tages-CSV von direwolf mitlesen (UTC-Datum im Namen); beim Start nur Neues, beim Tageswechsel die neue Datei.
    Kein Fehler darf den Thread beenden: kaputte Zeilen werden gemeldet und übersprungen."""
    cur, fh, header, buf, first = None, None, None, b"", True
    while True:
        try:
            name = os.path.join(LOGDIR, time.strftime("%Y-%m-%d.log", time.gmtime()))
            if name != cur and os.path.exists(name):
                if fh: fh.close()
                fh, cur, buf = open(name, "rb"), name, b""
                header = next(csv.reader([clean(fh.readline())]), None)
                if first: fh.seek(0, 2)               # beim Start alte Zeilen nicht noch einmal melden
            first = False                             # entsteht die Datei erst später: von vorn lesen
            chunk = fh.read() if fh else b""
            if chunk:
                buf += chunk
                *lines, buf = buf.split(b"\n")       # letzte (evtl. halbe) Zeile bleibt im Puffer
                for raw in lines:
                    try:
                        row = next(csv.reader([clean(raw)]), [])
                        if header and row and row[0] != "chan": emit(dict(zip(header, row)))
                    except Exception as e:
                        sys.stderr.write("aprs: Zeile übersprungen (%s): %r\n" % (e, raw[:120]))
            else:
                time.sleep(0.5)
        except Exception as e:
            sys.stderr.write("aprs: Mitlesen: %s\n" % e); time.sleep(2)


dw = subprocess.Popen(["direwolf", "-c", conf, "-r", RATE, "-b", "16", "-n", "1", "-t", "0", "-q", "hd", "-l", LOGDIR],
                      stdin=sys.stdin, stdout=subprocess.DEVNULL, stderr=sys.stderr)
reader = threading.Thread(target=tail, daemon=True); reader.start()


def summary():
    """aprs.json + relais.json für die Seite „Digital" (alle 60 s, liegt im Datenordner des Decoders)"""
    import aprssummary
    home = aprssummary.home_from_env()
    while True:
        try: aprssummary.run(LOGDIR, DATA, home)
        except Exception as e: sys.stderr.write("aprs: Übersicht: %s\n" % e)
        time.sleep(60)


threading.Thread(target=summary, daemon=True).start()
while dw.poll() is None:
    if not reader.is_alive():   # darf nicht vorkommen – dann lieber beenden, crabSDR startet den Decoder neu
        sys.stderr.write("aprs: Lese-Thread beendet, Decoder wird neu gestartet\n"); dw.kill(); sys.exit(1)
    time.sleep(5)
sys.exit(dw.returncode)
