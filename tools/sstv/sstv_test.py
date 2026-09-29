#!/usr/bin/env python3
"""sstv_test.py — Prüft den SSTV-Decoder (plugins/sstv/sstvdec.py) mit selbst erzeugten Signalen: Testbild mit scharfer
Schwarz-Weiß-Kante, VIS-Kopf, Scottie 1/2, Martin 1/2 und PD 120/90 (ISS), Sendetakt absichtlich verstimmt (Soundkarten weichen oft
einige Promille ab). Gemessen wird, wie weit die Kante in Rot, Grün und Blau auseinanderliegt (Farbsäume), und ob
die Kante an der richtigen Stelle sitzt.
   tools/sstv/sstv_test.py            -> Tabelle, Exit 1 wenn ein Kanal mehr als 1,5 Pixel daneben liegt
   tools/sstv/sstv_test.py --png DIR  -> decodierte Bilder zum Anschauen"""
import argparse, os, sys
import numpy as np
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "plugins", "sstv"))
import sstvdec

FS = 24000
EDGE = 160          # Kante: links schwarz, rechts weiß


def tone_seq(parts, fs, clock):
    """parts: Liste (Hz oder Array je Pixel, Dauer ms). Phasenstetig, Dauer mal clock (Sendetakt)."""
    out, ph, t_acc = [], 0.0, 0.0
    for f, ms in parts:
        n_float = ms * clock / 1000 * fs
        t_acc += n_float
        n = int(round(t_acc)) - sum(len(o) for o in out)
        if n <= 0: continue
        if np.isscalar(f):
            fr = np.full(n, float(f))
        else:
            fr = np.asarray(f, float)[(np.arange(n) * len(f) // n)]      # jedes Pixel gleich lang
        phase = ph + 2 * np.pi * np.cumsum(fr) / fs
        ph = phase[-1]
        out.append(np.sin(phase))
    return np.concatenate(out)


def vis_parts(vis):
    bits = [(vis >> k) & 1 for k in range(7)]; bits.append(sum(bits) % 2)
    p = [(1900, 300), (1200, 10), (1900, 300), (1200, 30)]
    p += [(1100 if b else 1300, 30) for b in bits]
    return p + [(1200, 30)]


def test_image(w, h):
    img = np.zeros((h, w, 3)); img[:, EDGE:, :] = 255
    return img


def pix(v): return 1500 + v / 255 * 800


def encode(vis, clock):
    m = sstvdec.MODES[vis]; W, H = m["w"], m["h"]; img = test_image(W, H)
    parts = [(1900, 200)] + vis_parts(vis)
    if m["kind"] == "scottie":
        parts.append((1200, m["sync"]))
        for r in range(H):
            parts += [(1500, m["sep"]), (pix(img[r, :, 1]), m["comp"]), (1500, m["sep"]), (pix(img[r, :, 2]), m["comp"]),
                      (1200, m["sync"]), (1500, m["porch"]), (pix(img[r, :, 0]), m["comp"])]
    elif m["kind"] == "martin":
        for r in range(H):
            parts += [(1200, m["sync"]), (1500, m["porch"])]
            for c in (1, 2, 0): parts += [(pix(img[r, :, c]), m["comp"]), (1500, m["gap"])]
    elif m["kind"] == "pd":   # Y(gerade), R-Y, B-Y, Y(ungerade); Grau: Y 16…235, Farbanteile 128
        yv = lambda r: 16 + img[r, :, 0] / 255 * 219
        for r in range(0, H, 2):
            parts += [(1200, m["sync"]), (1500, m["porch"]), (pix(yv(r)), m["comp"]), (pix(np.full(W, 128.0)), m["comp"]),
                      (pix(np.full(W, 128.0)), m["comp"]), (pix(yv(r + 1)), m["comp"])]
    parts.append((1900, 300))
    x = tone_seq(parts, FS, clock)
    return x + np.random.default_rng(1).normal(0, 0.05, len(x))


def edge_pos(row):
    """Spalte, an der der Kanal 128 überschreitet (linear interpoliert)."""
    i = int(np.argmax(row > 128))
    if i == 0: return float("nan")
    return i - 1 + (128 - row[i - 1]) / (row[i] - row[i - 1] + 1e-9)


def main():
    ap = argparse.ArgumentParser(); ap.add_argument("--png"); a = ap.parse_args()
    bad = 0
    print(f"{'Modus':<10} {'Takt':>7}  {'Kante R':>8} {'G':>7} {'B':>7}  {'Saum':>5}")
    for vis in (60, 56, 44, 40, 95, 99):
        for clock in (1.0, 1.003, 0.995, 1.01):
            r = sstvdec.decode(encode(vis, clock), FS)
            if not r: print(sstvdec.MODES[vis]["name"], clock, "nicht decodiert"); bad += 1; continue
            name, im, info = r
            arr = np.asarray(im).astype(float)
            rows = arr[int(arr.shape[0] * 0.3):int(arr.shape[0] * 0.7)]
            e = [np.nanmedian([edge_pos(row[:, c]) for row in rows]) for c in range(3)]
            spread = max(e) - min(e); off = np.mean(e) - (EDGE - 0.5)
            flag = spread > 1.5 or abs(off) > 3
            bad += flag
            print(f"{name:<10} {clock:>7.3f}  {e[0]:8.1f} {e[1]:7.1f} {e[2]:7.1f}  {spread:5.1f}{'  <-- Farbsaum' if flag else ''}")
            if a.png:
                os.makedirs(a.png, exist_ok=True); im.save(os.path.join(a.png, f"{name.replace(' ', '')}_{clock:.3f}.png"))
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
