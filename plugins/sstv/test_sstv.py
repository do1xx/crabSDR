#!/usr/bin/env python3
"""Prüfung des SSTV-Decoders gegen den Referenz-Encoder pysstv (pip install pysstv pillow numpy):
Testbild kodieren (Robot 36, Martin 1/2, Scottie 1, PD 120), 1,5 s Rauschen davor, dann so dekodieren, wie sstv.py es
tut (VIS suchen, Puffer ab 1 s vor VIS-Ende, decode(vis, t0)). Mittlerer Farbfehler je Kanal muss unter 10 bleiben.
   python3 plugins/sstv/test_sstv.py"""
import os, sys, numpy as np
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import sstvdec
from PIL import Image, ImageDraw
try:
    from pysstv.color import Robot36, MartinM1, MartinM2, ScottieS1, PD120
except ImportError:
    print("pysstv fehlt: pip install pysstv"); sys.exit(2)
FS = 24000


def testimg(w, h):
    im = Image.new("RGB", (w, h)); d = ImageDraw.Draw(im)
    for i, c in enumerate([(255, 0, 0), (0, 255, 0), (0, 0, 255), (255, 255, 0), (0, 255, 255), (255, 0, 255), (255, 255, 255), (60, 60, 60)]):
        d.rectangle([i * w // 8, 0, (i + 1) * w // 8, h // 2], fill=c)
    for y in range(h // 2, h):
        d.line([(0, y), (w, y)], fill=(int(255 * (y - h // 2) / (h // 2)), 128, 255 - int(255 * (y - h // 2) / (h // 2))))
    return im


def run(cls):
    im = testimg(cls.WIDTH, cls.HEIGHT)                                   # Encoder-Maß (pysstv Martin 2 = 160 px breit)
    pcm = np.array(list(cls(im, FS, 16).gen_samples()), dtype=np.float32) / 32768
    rng = np.random.default_rng(1)
    pre = rng.normal(0, 0.05, int(FS * 1.5)).astype(np.float32); post = rng.normal(0, 0.05, int(FS * 0.5)).astype(np.float32)
    x = np.concatenate([pre, pcm, post]) + rng.normal(0, 0.01, len(pre) + len(pcm) + len(post)).astype(np.float32)
    r = sstvdec.find_vis(x, FS)
    assert r, f"{cls.__name__}: VIS nicht gefunden"
    vis, end = r
    start = max(0, end - FS)                                             # wie sstv.py: Puffer ab 1 s vor VIS-Ende
    res = sstvdec.decode(x[start:], FS, vis=vis, t0=end - start)
    assert res, f"{cls.__name__}: kein Bild"
    name, out, info = res
    err = np.abs(np.asarray(im.resize(out.size), float) - np.asarray(out, float)).mean(axis=(0, 1))
    print(f"{name:10s} {info['lines']}/{info['of']} Zeilen, Farbfehler R/G/B {err.round(1).tolist()}")
    assert err.max() < 10, f"{name}: Farbfehler zu groß {err}"


if __name__ == "__main__":
    for cls in (Robot36, MartinM1, MartinM2, ScottieS1, PD120): run(cls)
    print("SSTV-Decoder in Ordnung")
