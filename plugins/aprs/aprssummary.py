#!/usr/bin/env python3
"""Übersicht für die Seite „Digital" aus den Tages-CSV von direwolf :
  aprs.json    letzte 300 Pakete + alle Stationen der letzten 90 Tage (direkt gehört / über Digi, letzte Position)
  relais.json  Relais, die per APRS ihre Frequenz melden (Symbol /r, Klubrufzeichen, „Repeater/Relais"), ≤ 250 km
Direkt oder über Digipeater: direwolf schreibt in „heard", wer das Paket über die Luft gesendet hat – der Absender
selbst (= direkt) oder der letzte Digi; „CALL?" = nur vermutet (WIDEn verbraucht), dann sicher nicht direkt.
Aufruf im Plugin alle 60 s; von Hand: aprssummary.py <logdir> <ausgabe-ordner> [lat lon]."""
import csv, glob, json, math, os, pickle, re, sys, time

DAYS = 90
NOTCALL = re.compile(r"^(WIDE|TRACE|RELAY|TCPIP|RFONLY|NOGATE|ECHO|GATE)", re.I)   # Pfad-Aliase, keine Stationen
REP = re.compile(r"repeater|relais|echolink|brandmeister", re.I)
CLUB = re.compile(r"^(?:ER-)?D[A-R]0[A-Z0-9]{1,3}\b")
MAXKM = 250     # weiter weg: nur über APRS-Digis gehört, das Relais selbst ist kaum zu empfangen


def num(v):
    try: return float(v) if v not in (None, "") else None
    except (TypeError, ValueError): return None


def pkt(r):
    src, h = (r.get("source") or "").strip(), (r.get("heard") or "").strip()
    lat, lon = num(r.get("latitude")), num(r.get("longitude"))
    if lat is None or lon is None: lat = lon = None   # kaputt formatierte Meldung -> keine Position
    return {"t": int(num(r.get("utime")) or 0), "src": src, "via": h, "direct": h == src, "digi": "" if h == src else h.rstrip("?"),
            "name": r.get("name", ""), "sym": r.get("symbol", ""), "lat": lat, "lon": lon,
            "alt": num(r.get("altitude")), "spd": num(r.get("speed")), "crs": num(r.get("course")),
            "lvl": r.get("level", ""), "err": r.get("error", ""), "dti": r.get("dti", ""),
            "status": r.get("status", ""), "comment": r.get("comment", "")}


def rows(path):
    try:
        with open(path, newline="", errors="replace") as fh:
            return list(csv.DictReader(fh))
    except OSError:
        return []


def file_stations(path):
    st = {}
    def get(c):
        return st.setdefault(c, {"src": c, "n": 0, "n_direct": 0, "last": 0, "last_direct": 0, "relayed": 0, "digis": {}})
    for r in rows(path):
        p = pkt(r)
        if not p["src"] or NOTCALL.match(p["src"]):
            continue
        s = get(p["src"]); s["n"] += 1; s["last"] = max(s["last"], p["t"])
        if p["direct"]:
            s["n_direct"] += 1; s["last_direct"] = max(s["last_direct"], p["t"])
        elif p["digi"]:
            s["digis"][p["digi"]] = s["digis"].get(p["digi"], 0) + 1
        if p["lat"] is not None and p["name"] in ("", p["src"]) and p["t"] >= s.get("pos_t", 0):
            s.update({"lat": p["lat"], "lon": p["lon"], "sym": p["sym"], "comment": p["comment"], "pos_t": p["t"]})
        if p["via"] and not p["via"].endswith("?") and not p["direct"] and not NOTCALL.match(p["via"]):   # Digi direkt gehört
            d = get(p["via"]); d["relayed"] += 1
            d["last_direct"] = max(d["last_direct"], p["t"]); d["last"] = max(d["last"], p["t"])
    return st


def dump(obj, path):
    tmp = path + ".tmp"
    with open(tmp, "w") as f: json.dump(obj, f, ensure_ascii=False)
    os.replace(tmp, path)


def run(logdir, out, home=None):
    os.makedirs(out, exist_ok=True)
    files = sorted(glob.glob(os.path.join(logdir, "*.log")))[-DAYS:]
    recent = []
    for f in files[-2:]:
        recent += rows(f)
    pk = [pkt(r) for r in recent[-300:]]
    # Stationen je Tagesdatei zusammengefasst und zwischengespeichert (nur geänderte Dateien neu lesen)
    cache_f = os.path.join(out, "aprs-stations.cache")
    try: cache = pickle.load(open(cache_f, "rb"))
    except Exception: cache = {}
    newcache = {}
    for f in files:
        try:
            key = (os.path.getmtime(f), os.path.getsize(f)); c = cache.get(f)
            newcache[f] = c if c and c[0] == key else (key, file_stations(f))
        except OSError:
            pass
    try:
        pickle.dump(newcache, open(cache_f + ".tmp", "wb")); os.replace(cache_f + ".tmp", cache_f)
    except OSError:
        pass
    stations = {}
    for f in files:
        for c, x in (newcache.get(f) or (None, {}))[1].items():
            s = stations.setdefault(c, {"src": c, "n": 0, "n_direct": 0, "last": 0, "last_direct": 0, "relayed": 0, "digis": {}})
            s["n"] += x["n"]; s["n_direct"] += x["n_direct"]; s["relayed"] += x["relayed"]
            s["last"] = max(s["last"], x["last"]); s["last_direct"] = max(s["last_direct"], x["last_direct"])
            for k, v in x["digis"].items(): s["digis"][k] = s["digis"].get(k, 0) + v
            if "lat" in x and x.get("pos_t", -1) >= s.get("pos_t", -1):
                s.update({k: x[k] for k in ("lat", "lon", "sym", "comment", "pos_t")})
    for s in stations.values():
        s["direct"] = bool(s["n_direct"] or s["relayed"])
        s["digis"] = sorted(s["digis"], key=lambda k: -s["digis"][k])[:3]
        if not s["relayed"]: del s["relayed"]
    dump({"ts": int(time.time()), "days": DAYS, "packets": pk[::-1], "stations": sorted(stations.values(), key=lambda s: -s["last"])},
         os.path.join(out, "aprs.json"))

    # Relais aus den letzten 60 Tagen
    def dist(lat, lon):
        if not home: return 0
        p1, p2 = math.radians(home[0]), math.radians(lat); dl = math.radians(lon - home[1])
        return 6371 * math.acos(max(-1, min(1, math.sin(p1) * math.sin(p2) + math.cos(p1) * math.cos(p2) * math.cos(dl))))
    rel = {}
    for f in files[-60:]:
        for r in rows(f):
            mhz = num(r.get("frequency"))
            if not mhz: continue
            name, com, sym = (r.get("name") or "").strip(), (r.get("comment") or "").strip(), r.get("symbol") or ""
            if not (sym.startswith("/r") or CLUB.match(name) or REP.search(com)): continue
            call = re.sub(r"^ER-", "", name.split()[0]) if name else ""
            if re.match(r"^\d", call): call = ""      # Objektname ist nur die Frequenz: Rufzeichen unbekannt
            lat, lon = num(r.get("latitude")), num(r.get("longitude"))
            if lat is None or lon is None or dist(lat, lon) > MAXKM: continue
            k = round(mhz * 1000, 1)
            e = rel.setdefault(k, {"khz": k, "call": call, "n": 0})
            e.update({"offset": r.get("offset") or e.get("offset", ""), "tone": r.get("tone") or e.get("tone", ""),
                      "lat": lat, "lon": lon, "km": round(dist(lat, lon)) if home else None, "comment": com[:80],
                      "last": int(num(r.get("utime")) or 0), "src": r.get("source", "")})
            if call and not e["call"]: e["call"] = call
            e["n"] += 1
    for e in rel.values():
        if e.get("tone") in ("0.0", "0"): e["tone"] = ""
    dump({"ts": int(time.time()), "relais": sorted(rel.values(), key=lambda e: e["khz"])}, os.path.join(out, "relais.json"))


def home_from_env():
    la, lo = num(os.environ.get("CRAB_STATION_LAT")), num(os.environ.get("CRAB_STATION_LON"))
    return (la, lo) if la is not None and lo is not None else None


if __name__ == "__main__":
    a = sys.argv[1:]
    run(a[0], a[1], (float(a[2]), float(a[3])) if len(a) >= 4 else home_from_env())
