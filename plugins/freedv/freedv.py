#!/usr/bin/env python3
"""crabSDR-Decoder FreeDV: liest USB-Ton (s16le mono, 8000 Hz) von stdin und dekodiert mit libcodec2 (FreeDV-API per ctypes)
die Codec-2-Modes 700D, 700E und 1600 parallel. Die Sprache des Modes, der gerade Sync hat, geht als s16le 8 kHz auf stdout
(crabSDR verteilt sie als hörbaren Kanal). Treffer als JSON auf stderr: kind "sync" (mode, snr, on) beim Kommen und Gehen
des Signals, kind "text" (Rufzeichen/Text aus dem FreeDV-Textkanal). Übersicht freedv.json im Datenordner.
Optionen: modes (Standard "700D,700E,1600"), squelch (SNR-Schwelle dB, Standard Mode-Vorgabe), snr_min (Sync zählt erst ab
so viel dB, Standard 2; Rauschen lässt den Sync sonst im Sekundentakt flattern), hold (s, Standard 1.0: so lange muss der
Sync stehen, bevor er gemeldet wird, und so lange bleibt er nach dem Verlust).
Braucht libcodec2 ≥ 1.0 (Debian: libcodec2-1.2).
"""
import ctypes, ctypes.util, json, os, sys, time
DATA = os.environ.get("CRAB_DATA", "."); RATE = int(os.environ.get("CRAB_RATE", "8000"))
MODES = {"1600": 0, "700C": 6, "700D": 7, "700E": 13, "2400A": 3, "2400B": 4, "800XA": 5}
WANT = [m.strip().upper() for m in os.environ.get("CRAB_OPT_MODES", "700D,700E,1600").split(",") if m.strip().upper() in MODES]
SQUELCH = os.environ.get("CRAB_OPT_SQUELCH")
SNR_MIN = float(os.environ.get("CRAB_OPT_SNR_MIN", "2"))
HOLD = float(os.environ.get("CRAB_OPT_HOLD", "1.0"))
OUT = os.path.join(DATA, "freedv.json"); os.makedirs(DATA, exist_ok=True)

def say(ev):
    sys.stderr.write(json.dumps(ev, ensure_ascii=False) + "\n"); sys.stderr.flush()

lib_name = ctypes.util.find_library("codec2") or "libcodec2.so.1.2"
try:
    lib = ctypes.CDLL(lib_name)
except OSError as e:
    sys.stderr.write("freedv: libcodec2 fehlt (%s) – apt install libcodec2-1.2\n" % e); sys.exit(1)
lib.freedv_open.restype = ctypes.c_void_p; lib.freedv_open.argtypes = [ctypes.c_int]
lib.freedv_close.argtypes = [ctypes.c_void_p]
lib.freedv_nin.restype = ctypes.c_int; lib.freedv_nin.argtypes = [ctypes.c_void_p]
lib.freedv_rx.restype = ctypes.c_int; lib.freedv_rx.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_short), ctypes.POINTER(ctypes.c_short)]
lib.freedv_get_n_max_modem_samples.restype = ctypes.c_int; lib.freedv_get_n_max_modem_samples.argtypes = [ctypes.c_void_p]
lib.freedv_get_n_max_speech_samples.restype = ctypes.c_int; lib.freedv_get_n_max_speech_samples.argtypes = [ctypes.c_void_p]
lib.freedv_get_modem_sample_rate.restype = ctypes.c_int; lib.freedv_get_modem_sample_rate.argtypes = [ctypes.c_void_p]
lib.freedv_get_speech_sample_rate.restype = ctypes.c_int; lib.freedv_get_speech_sample_rate.argtypes = [ctypes.c_void_p]
lib.freedv_get_sync.restype = ctypes.c_int; lib.freedv_get_sync.argtypes = [ctypes.c_void_p]
lib.freedv_get_modem_stats.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_int), ctypes.POINTER(ctypes.c_float)]
lib.freedv_set_squelch_en.argtypes = [ctypes.c_void_p, ctypes.c_bool]
lib.freedv_set_snr_squelch_thresh.argtypes = [ctypes.c_void_p, ctypes.c_float]
CB = ctypes.CFUNCTYPE(None, ctypes.c_void_p, ctypes.c_char)
lib.freedv_set_callback_txt.argtypes = [ctypes.c_void_p, CB, ctypes.c_void_p, ctypes.c_void_p]

class Rx:
    def __init__(self, name):
        self.name = name; self.f = lib.freedv_open(MODES[name])
        if not self.f: raise RuntimeError("freedv_open %s" % name)
        self.nmax = lib.freedv_get_n_max_modem_samples(self.f); self.smax = lib.freedv_get_n_max_speech_samples(self.f)
        self.mrate = lib.freedv_get_modem_sample_rate(self.f); self.srate = lib.freedv_get_speech_sample_rate(self.f)
        self.inbuf = (ctypes.c_short * self.nmax)(); self.out = (ctypes.c_short * self.smax)()
        self.fill = 0; self.sync = 0; self.snr = 0.0; self.text = ""
        self.on = False; self.since = None; self.lost = None; self.t_audio = 0.0
        self.last_text = ("", 0.0)   # Textkanal wiederholt sich im Kreis: gleicher Text höchstens alle 60 s   # entprellter Sync nach Tonzeit (nicht Wanduhr): an nach HOLD s stabil, aus nach HOLD s ohne
        if SQUELCH: lib.freedv_set_snr_squelch_thresh(self.f, float(SQUELCH))
        lib.freedv_set_squelch_en(self.f, True)
        self.cb = CB(self._txt); lib.freedv_set_callback_txt(self.f, self.cb, None, None)
    def _txt(self, _st, ch):
        c = ch.decode("latin-1", "replace") if isinstance(ch, bytes) else chr(ch)
        if c in "\r\n":
            t = self.text.strip()[:80]
            if t and (t != self.last_text[0] or self.t_audio - self.last_text[1] >= 60):
                say({"kind": "text", "mode": self.name, "text": t}); self.last_text = (t, self.t_audio)
            self.text = ""
        elif c.isprintable(): self.text = (self.text + c)[-80:]
    def feed(self, samples):
        """Abtastwerte (Liste von int) verarbeiten, dekodierte Sprache als bytes zurück (leer ohne Sync)"""
        speech = bytearray(); i = 0; n = len(samples)
        while i < n:
            need = lib.freedv_nin(self.f) - self.fill
            take = min(need, n - i)
            self.inbuf[self.fill:self.fill + take] = samples[i:i + take]
            self.fill += take; i += take
            if self.fill >= lib.freedv_nin(self.f):
                take_total = self.fill
                nout = lib.freedv_rx(self.f, self.out, self.inbuf); self.fill = 0
                s = ctypes.c_int(0); snr = ctypes.c_float(0.0); lib.freedv_get_modem_stats(self.f, ctypes.byref(s), ctypes.byref(snr))
                self.sync, self.snr = s.value, snr.value
                self.t_audio += take_total / float(self.mrate)
                now = self.t_audio; good = bool(self.sync) and self.snr >= SNR_MIN
                if good:
                    if self.since is None: self.since = now
                    self.lost = None
                    if not self.on and now - self.since >= HOLD: self.on = True
                else:
                    self.since = None
                    if self.on:
                        if self.lost is None: self.lost = now
                        elif now - self.lost >= HOLD: self.on = False
                if nout > 0 and (self.on or good): speech += bytes(bytearray(self.out)[:nout * 2])
        return bytes(speech)

rxs = [Rx(m) for m in WANT]
if not rxs: sys.stderr.write("freedv: keine gültigen Modes\n"); sys.exit(1)
if rxs[0].mrate != RATE: sys.stderr.write("freedv: Modem will %d Hz, crabSDR liefert %d – rate im Plugin anpassen\n" % (rxs[0].mrate, RATE))
sys.stderr.write("freedv: libcodec2 %s, Modes %s\n" % (lib_name, ",".join(WANT)))
active = None; last_state = {r.name: False for r in rxs}; last_json = 0
def write_json():
    tmp = OUT + ".tmp"
    with open(tmp, "w") as f:
        json.dump({"ts": int(time.time()), "active": active, "modes": [{"mode": r.name, "sync": r.on, "snr": round(r.snr, 1)} for r in rxs]}, f)
    os.replace(tmp, OUT)
fin = sys.stdin.buffer; fout = sys.stdout.buffer
while True:
    chunk = fin.read(640)   # 40 ms
    if not chunk: break
    if len(chunk) % 2: chunk = chunk[:-1]
    samples = list(memoryview(chunk).cast("h"))
    outs = {r.name: r.feed(samples) for r in rxs}
    synced = [r for r in rxs if r.on]
    # Mode-Wahl: der mit (entprelltem) Sync und bestem SNR; Wechsel nur, wenn der aktive Mode den Sync verloren hat
    if active and any(r.name == active and r.on for r in rxs): pass
    else: active = max(synced, key=lambda r: r.snr).name if synced else None
    for r in rxs:
        if last_state.get(r.name) != r.on:
            say({"kind": "sync", "mode": r.name, "on": r.on, "snr": round(r.snr, 1)})
        last_state[r.name] = r.on
    if active and outs.get(active): fout.write(outs[active]); fout.flush()
    now = time.time()
    if now - last_json > 2: last_json = now; write_json()
