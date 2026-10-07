# Tests

**Rust** (Kanalfilter, Demodulator, Konfiguration, Golden-Test gegen ein synthetisches Signal):

```bash
python3 tools/phase0/synth.py testdata/synth.cu8 --sec 30   # Testsignal für den Golden-Test (sonst übersprungen)
cd backend-rs && cargo test --workspace
```

**Oberfläche** (Playwright, Chromium): `run.sh` erzeugt das Testsignal, spielt es mit `tools/phase0/iqplay.py` als
`rtl_tcp` ab, startet zwei Testserver (`testdata/config-ui.toml` auf 8082, `config-ui2.toml` auf 8083) und führt alle
Prüfungen aus.

```bash
cd tools/e2e && npm install && npx playwright install chromium && cd ../..
tools/e2e/run.sh
```

| Test | prüft |
|---|---|
| `ui.spec.mjs` | Bänder, Wasserfälle, Abstimmen, Zoom, Ton (Opus), Aufnahme |
| `bandpick.spec.mjs` | Bänder ein-/ausschalten, Bildschirm teilen, Klick-Abstimmung mit Raster |
| `squelch.spec.mjs` | Rauschsperre (FM startet mit Sperre, Links mit Betriebsart) |
| `share.spec.mjs` | Teilen-Link mit Frequenz und Betriebsart |
| `decmenu.spec.mjs` | Decoder-Menü |
| `features.spec.mjs` | Schalter in `[ui]`: Reiter, Chat, Banner, Impressum |
| `package-smoke.mjs` | Docker-Image/Release-Paket von außen (`node package-smoke.mjs http://host:8080`) |
| `decoder-test.sh` | APRS und FT8 Ende zu Ende mit erzeugten Signalen im vollen Docker-Image |

Werkzeuge in `tools/phase0/`: `iqplay.py` (.cu8 als rtl_tcp), `synth.py` (Testsignal), `wsclient.py`
(Protokoll-Testclient), `refdemod.py`/`audiostats.py` (Referenz-Demodulator, Kennzahlen), `iqrec-remote.py`
(Mitschnitt von einem laufenden rtl_tcp).
