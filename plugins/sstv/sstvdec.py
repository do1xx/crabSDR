#!/usr/bin/env python3
"""sstvdec.py — SSTV-Decoder in numpy (kein Soundgerät, keine GUI). Erkennt den Modus am VIS-Kopf und decodiert
Martin 1/2, Scottie 1/2/DX, Robot 36/72, PD 50/90/120/160/180/240/290 (ISS sendet PD120). Zeilensynchron über die
1200-Hz-Syncimpulse, daher robust gegen kleine Taktabweichungen.
   sstvdec.py aufnahme.wav bild.png      -> JSON mit Modus, Zeilen, Startzeit
Als Modul: find_vis(x, fs) -> (vis, t0_s) | None ;  decode(x, fs, vis=None) -> (modename, PIL.Image, info) | None
"""
import json, sys, wave
import numpy as np

F_BLACK, F_WHITE, F_SYNC = 1500.0, 2300.0, 1200.0

MODES = {
    44: dict(name="Martin 1",  kind="martin",  w=320, h=256, sync=4.862, porch=0.572, comp=146.432, gap=0.572),
    40: dict(name="Martin 2",  kind="martin",  w=320, h=256, sync=4.862, porch=0.572, comp=73.216,  gap=0.572),
    60: dict(name="Scottie 1", kind="scottie", w=320, h=256, sync=9.0,   porch=1.5,   comp=138.24,  sep=1.5),
    56: dict(name="Scottie 2", kind="scottie", w=320, h=256, sync=9.0,   porch=1.5,   comp=88.064,  sep=1.5),
    76: dict(name="Scottie DX",kind="scottie", w=320, h=256, sync=9.0,   porch=1.5,   comp=345.6,   sep=1.5),
    8:  dict(name="Robot 36",  kind="robot36", w=320, h=240, sync=9.0,   porch=3.0,   y=88.0, sep=4.5, porch2=1.5, c=44.0),
    12: dict(name="Robot 72",  kind="robot72", w=320, h=240, sync=9.0,   porch=3.0,   y=138.0, sep=4.5, porch2=1.5, c=69.0),
    93: dict(name="PD 50",     kind="pd",      w=320, h=256, sync=20.0,  porch=2.08,  comp=91.52),
    99: dict(name="PD 90",     kind="pd",      w=320, h=256, sync=20.0,  porch=2.08,  comp=170.24),
    95: dict(name="PD 120",    kind="pd",      w=640, h=496, sync=20.0,  porch=2.08,  comp=121.6),
    98: dict(name="PD 160",    kind="pd",      w=512, h=400, sync=20.0,  porch=2.08,  comp=195.584),
    96: dict(name="PD 180",    kind="pd",      w=640, h=496, sync=20.0,  porch=2.08,  comp=183.04),
    97: dict(name="PD 240",    kind="pd",      w=640, h=496, sync=20.0,  porch=2.08,  comp=244.48),
    94: dict(name="PD 290",    kind="pd",      w=800, h=616, sync=20.0,  porch=2.08,  comp=228.8),
}


def line_ms(m):
    k = m["kind"]
    if k == "martin":  return m["sync"] + m["porch"] + 3 * (m["comp"] + m["gap"])
    if k == "scottie": return 3 * m["comp"] + 2 * m["sep"] + m["sync"] + m["porch"]
    if k == "robot36": return m["sync"] + m["porch"] + m["y"] + m["sep"] + m["porch2"] + m["c"]
    if k == "robot72": return m["sync"] + m["porch"] + m["y"] + 2 * (m["sep"] + m["porch2"] + m["c"])
    return m["sync"] + m["porch"] + 4 * m["comp"]


def duration_s(vis):
    """Gesamtdauer eines Bildes ab VIS-Ende (für die Aufnahmelänge)."""
    m = MODES.get(vis)
    if not m: return 130.0
    lines = m["h"] / 2 if m["kind"] == "pd" else m["h"]
    return lines * line_ms(m) / 1000 + (m["sync"] / 1000 if m["kind"] == "scottie" else 0) + 1.0


def freq_track(x, fs):
    """Momentanfrequenz (Hz) je Sample über das analytische Signal (FFT-Hilbert, keine Spiegelfrequenzen),
    danach leichte Glättung (~0,3 ms)."""
    x = np.asarray(x, dtype=np.float64)
    n = len(x)
    X = np.fft.rfft(x)
    # analytisches Signal: einseitiges Spektrum
    h = np.zeros(n // 2 + 1); h[0] = 1.0; h[1:] = 2.0
    if n % 2 == 0: h[-1] = 1.0
    xa = np.fft.ifft(np.concatenate((X * h, np.zeros(n - (n // 2 + 1), dtype=complex))))[:n] if False else np.fft.irfft(X * h, n) + 1j * np.fft.irfft(X * h * (-1j) * np.sign(np.arange(n // 2 + 1)), n)
    ph = np.angle(xa[1:] * np.conj(xa[:-1]))
    f = ph * fs / (2 * np.pi)
    f = np.append(f, f[-1])
    k = max(3, int(fs * 0.0003))
    return np.convolve(f, np.ones(k) / k, mode="same")


def find_vis(x, fs, f=None, start=0):
    """Sucht den VIS-Kopf: >=150 ms 1900 Hz, Startbit 1200 Hz (30 ms), 8 Bits (1100=1/1300=0, LSB zuerst, Bit 7 = gerade Parität), Stopbit.
    Liefert (vis, index des VIS-Endes) oder None."""
    if f is None: f = freq_track(x, fs)
    b = max(1, int(fs * 0.001))                      # 1-ms-Blöcke
    nb = len(f) // b
    blk = np.median(f[:nb * b].reshape(nb, b), axis=1)
    is1200 = np.abs(blk - 1200) < 50           # eng: 1100/1300 sind die VIS-Bits, keine Syncs
    is1900 = np.abs(blk - 1900) < 120
    i = max(start // b, 160)
    while i < nb - 330:
        if is1200[i] and not is1200[i - 1]:
            j = i
            while j < nb and is1200[j]: j += 1
            run = j - i
            if 18 <= run <= 45 and is1900[i - 150:i - 5].mean() > 0.6:
                bits = []
                for k in range(8):
                    c = j + 15 + 30 * k
                    v = np.median(blk[c - 6:c + 6])
                    bits.append(1 if v < 1200 else 0)
                vis = sum(bit << k for k, bit in enumerate(bits[:7]))
                par = sum(bits[:7]) % 2
                stop = j + 30 * 8
                if par == bits[7] and is1200[stop + 3:stop + 27].mean() > 0.6:
                    return vis, (stop + 30) * b
            i = j
        else:
            i += 1
    return None


def _sync_runs(f, fs):
    m = (np.abs(f - 1200) < 80).astype(np.int8)   # Sync 1200 Hz; 1100/1300 (VIS-Bits) und 1500 (Schwarz) bleiben draußen
    d = np.diff(np.concatenate(([0], m, [0])))
    starts = np.where(d == 1)[0]; ends = np.where(d == -1)[0]
    return starts, ends


def _find_sync(starts, ends, expected, window, min_len):
    lo, hi = expected - window, expected + window
    best = None
    for s, e in zip(starts, ends):
        if s < lo: continue
        if s > hi: break
        if e - s >= min_len and (best is None or abs(s - expected) < abs(best - expected)):
            best = s
    return best


def _pixels(f, t_start, dur_ms, w, fs):
    """Mittelwert der Frequenz je Pixel -> 0..255."""
    edges = t_start + np.arange(w + 1) * (dur_ms / 1000 * fs / w)
    e = np.clip(edges.astype(np.int64), 0, len(f) - 1)
    cs = np.concatenate(([0.0], np.cumsum(f)))
    n = np.maximum(e[1:] - e[:-1], 1)
    v = (cs[e[1:]] - cs[e[:-1]]) / n
    return np.clip((v - F_BLACK) / (F_WHITE - F_BLACK) * 255.0, 0, 255)


def _ycc(y, cr, cb):
    y = 1.164 * (y - 16.0); cr = cr - 128.0; cb = cb - 128.0
    r = y + 1.596 * cr; g = y - 0.813 * cr - 0.392 * cb; bl = y + 2.017 * cb
    return np.clip(np.stack([r, g, bl], axis=-1), 0, 255)


def decode(x, fs, vis=None, f=None):
    from PIL import Image
    if f is None: f = freq_track(x, fs)
    t0 = 0
    if vis is None:
        r = find_vis(x, fs, f)
        if not r: return None
        vis, t0 = r
    m = MODES.get(vis)
    if not m: return None
    ms = fs / 1000.0
    W, H, kind = m["w"], m["h"], m["kind"]
    T = line_ms(m) * ms
    starts, ends = _sync_runs(f, fs)
    sync_len = m["sync"] * ms
    img = np.zeros((H, W, 3), dtype=np.float32)
    rows = H // 2 if kind == "pd" else H
    if kind == "scottie":
        first = t0 + (m["sync"] + m["sep"] + m["comp"] + m["sep"] + m["comp"]) * ms   # Sync der 1. Zeile sitzt vor Rot
    else:
        first = t0
    expected = first; decoded = 0; last = None
    for row in range(rows):
        if last is None:
            s = _find_sync(starts, ends, expected + int(T * 0.3), int(T * 0.3) + int(5 * ms), sync_len * 0.5)   # erster Sync: ab VIS-Ende, nicht davor
        else:
            s = _find_sync(starts, ends, expected, int(min(T * 0.25, 40 * ms)), sync_len * 0.5)
        if s is None: s = expected
        if s + T > len(f) + 5 * ms: break
        last = s; expected = s + T
        if kind == "martin":
            p = s + (m["sync"] + m["porch"]) * ms; step = (m["comp"] + m["gap"]) * ms
            g = _pixels(f, p, m["comp"], W, fs); b = _pixels(f, p + step, m["comp"], W, fs); r = _pixels(f, p + 2 * step, m["comp"], W, fs)
            img[row] = np.stack([r, g, b], axis=-1)
        elif kind == "scottie":
            r = _pixels(f, s + (m["sync"] + m["porch"]) * ms, m["comp"], W, fs)
            b = _pixels(f, s - m["comp"] * ms, m["comp"], W, fs)
            g = _pixels(f, s - (m["comp"] + m["sep"] + m["comp"]) * ms, m["comp"], W, fs)
            img[row] = np.stack([r, g, b], axis=-1)
        elif kind == "robot36":
            p = s + (m["sync"] + m["porch"]) * ms
            y = _pixels(f, p, m["y"], W, fs)
            c = _pixels(f, p + (m["y"] + m["sep"] + m["porch2"]) * ms, m["c"], W // 2, fs)
            c = np.repeat(c, 2)
            if row % 2 == 0:
                img[row, :, 0] = y; img[row, :, 1] = c            # R-Y in Kanal 1 zwischenspeichern
            else:
                cr = img[row - 1, :, 1].copy()
                img[row - 1] = _ycc(img[row - 1, :, 0].copy(), cr, c); img[row] = _ycc(y, cr, c)
        elif kind == "robot72":
            p = s + (m["sync"] + m["porch"]) * ms
            y = _pixels(f, p, m["y"], W, fs)
            q = p + (m["y"] + m["sep"] + m["porch2"]) * ms
            cr = np.repeat(_pixels(f, q, m["c"], W // 2, fs), 2)
            cb = np.repeat(_pixels(f, q + (m["c"] + m["sep"] + m["porch2"]) * ms, m["c"], W // 2, fs), 2)
            img[row] = _ycc(y, cr, cb)
        else:   # pd: Y(gerade), R-Y, B-Y, Y(ungerade)
            p = s + (m["sync"] + m["porch"]) * ms; step = m["comp"] * ms
            y0 = _pixels(f, p, m["comp"], W, fs); cr = _pixels(f, p + step, m["comp"], W, fs)
            cb = _pixels(f, p + 2 * step, m["comp"], W, fs); y1 = _pixels(f, p + 3 * step, m["comp"], W, fs)
            img[2 * row] = _ycc(y0, cr, cb); img[2 * row + 1] = _ycc(y1, cr, cb)
        decoded += 1
    if decoded < rows * 0.25:
        return None
    if kind == "robot36" and decoded % 2 == 1:            # letzte gerade Zeile ohne Partner
        img[decoded - 1] = _ycc(img[decoded - 1, :, 0].copy(), img[decoded - 1, :, 1].copy(), np.full(W, 128.0))
    im = Image.fromarray(img.astype(np.uint8), "RGB")
    return m["name"], im, {"vis": vis, "lines": decoded * (2 if kind == "pd" else 1), "of": H, "t0": t0 / fs}


def read_wav(path):
    w = wave.open(path, "rb"); fs = w.getframerate(); n = w.getnframes(); ch = w.getnchannels(); sw = w.getsampwidth()
    raw = w.readframes(n); w.close()
    if sw == 2: x = np.frombuffer(raw, dtype=np.int16).astype(np.float32) / 32768
    elif sw == 1: x = (np.frombuffer(raw, dtype=np.uint8).astype(np.float32) - 128) / 128
    else: x = np.frombuffer(raw, dtype=np.int32).astype(np.float32) / 2 ** 31
    if ch > 1: x = x.reshape(-1, ch).mean(axis=1)
    return x, fs


if __name__ == "__main__":
    if len(sys.argv) < 3:
        print(__doc__); sys.exit(2)
    x, fs = read_wav(sys.argv[1])
    r = decode(x, fs)
    if not r:
        print(json.dumps({"ok": False})); sys.exit(1)
    name, im, info = r
    im.save(sys.argv[2])
    print(json.dumps({"ok": True, "mode": name, **info}))
