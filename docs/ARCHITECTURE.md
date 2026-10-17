# crabSDR – Aufbau

Ein Programm (`crabsdr-server`, Rust) liest die Empfänger, rechnet alles selbst und liefert die Weboberfläche aus.
Jeder Hörer bekommt seinen eigenen Kanal (Frequenz, Betriebsart, Bandbreite), Decoder hören als unsichtbare Hörer mit.
Gezoomte Wasserfall-Ausschnitte jenseits von 1 Bin je Pixel bekommen ein eigenes Zoom-Spektrum (`ZoomSpectrum`: Ausschnitt
wie ein Kanal ausgeschnitten, 2048er-FFT), damit die Auflösung mit dem Zoom steigt.

```
 Stick / rtl_tcp ──IQ──►  Band-Pipeline ──►  DSP-Thread ──┬─► Wasserfall-Zeilen ─────────┐
 (je Band einer)          (Treiber, Wächter)  FFT ~500 Hz  ├─► Kanäle je Frequenz ──► Opus ├─► WebSocket /ws/<band>
                                              50 Zeilen/s  │   (Hörer teilen Kanäle)       │   (Browser)
                                                           └─► Rohton ──► Decoder-Plugins ─┴─► /api/decoders, MQTT
```

## Rust-Crates (`backend-rs/crates/`)

| Crate | Aufgabe |
|---|---|
| `crabsdr-core` | Konfiguration (`config.rs`: Laden, Prüfen, Voreinstellungen), Protokoll (Binärrahmen), gemeinsame Typen |
| `crabsdr-sdr` | Quellen: `rtl_sdr`, `rtl_tcp`, `hackrf`, `rx_sdr` (SoapySDR) als Unterprozess bzw. TCP; Wächter startet hängende Quellen neu |
| `crabsdr-dsp` | FFT-Kanalfilter (Overlap-Save, Phase über Blöcke fortgesetzt), Demodulatoren FM/AM/SSB/CW, Rauschsperre, AGC, Resampler |
| `crabsdr-auth` | optionale Benutzerverwaltung (SQLite, JWT): Admin, Nutzer, Gäste |
| `crabsdr-server` | Webserver (axum): WebSocket je Band, DSP-Thread, Hörerverwaltung, Decoder, Chat, Seiten-Daten, `--check` |

### Wichtige Dateien im Server

| Datei | Inhalt |
|---|---|
| `main.rs` | Start, Kommandozeile, Routen, WebSocket-Befehle, `ui.json`/`bandinfo.js` für die Oberfläche |
| `sdr_pipeline.rs`, `sdr_manager.rs` | eine Pipeline je Band: Quelle, DSP-Thread, Hörer, Zustand |
| `dsp_thread.rs` | FFT, Wasserfall (inkl. 2 min Verlauf als JPEG beim Anmelden), Kanäle gruppiert nach Frequenz/Modus/Bandbreite |
| `client.rs` | Zustand je Hörer; Hörer mit gleichem `ChannelKey` teilen Filter, Demodulator und Opus-Encoder |
| `decoders.rs` | `[[decoders]]`: Plugin starten, Rohton zuführen, Meldungen (JSON-Zeilen) sammeln, Neustart mit Pause, MQTT |
| `chat.rs` | Chat (SQLite `pinnwand.db`, Pfade aus alter Zeit unter `/logbuch/api/`), Hörer online |
| `digi.rs` | Daten der Digital-Seite aus den Datenordnern der Decoder |
| `check.rs` | `crabsdr-server --check` |
| `auth.rs`, `access.rs`, `ratelimit.rs` | Anmeldung, Rechte je Band und Decoder, Anmeldebremse ([SECURITY.md](SECURITY.md)) |
| `admin.rs`, `confedit.rs` | Admin-Schnittstelle: Überblick, Benutzer, Konfiguration bearbeiten (Kommentare bleiben, Sicherungen) |
| `directory.rs` | optionaler Eintrag im Verzeichnis crabsdr.de (`[directory]`) |

## Kosten

Ein Kanal wird nur einmal gerechnet, egal wie viele Hörer ihn hören. Gemessen auf x86: jedes Band mit 2,048 MS/s
etwa 4 % CPU, jede weitere belegte Frequenz etwa 0,4 %. Ein Raspberry Pi 4 trägt zwei Bänder mit APRS und FT8.

## WebSocket `/ws/<band>`

Steuerung als JSON (Text), Daten als Binärrahmen mit einem Kennbyte vorn:

| Kennbyte | Inhalt |
|---|---|
| `0x82` | Opus-Ton (24 kHz mono) |
| `0x02` | PCM-Ton (s16le), wenn der Browser kein WebCodecs hat |
| `0x84` | Wasserfall-Zeile (Zoomstufe, Start-Bin, Differenz- oder Absolutzeile) |
| `0x85` / `0x86` | Wasserfall-Verlauf beim Anmelden (Zeilenblock bzw. fertiges JPEG) |
| `0x03` | JSON vom Server (Konfiguration, Pegel, Hörerliste) |

Befehle vom Browser: `tune` (Frequenz, Modus, Bandbreite), `untune`, `set_squelch`, `set_agc`, `set_codec`,
`set_waterfall` (Zoom, Ausschnitt, Verlauf), `set_name`; auf Admin-Bändern für Admins `set_gain` und `set_center_freq`.

## HTTP

| Pfad | Inhalt |
|---|---|
| `/`, `/digi/`, `/info/` | Oberfläche (`web/`, Stationsordner hat Vorrang) |
| `/bandinfo.js`, `/ui.json` | Bänder und Einstellungen für die Oberfläche |
| `/api/health` | Zustand, Version, Laufzeit |
| `/api/bands`, `/api/listeners` | Bänder, Hörer (`n` Personen, `n_audio` hören gerade, `connections` Verbindungen) |
| `/api/decoders`, `/api/decoders/events?since=&wait=1` | Decoder-Zustand, Meldungen (Langabfrage) |
| `/digi/aprs.json`, `ft8.json`, `sstv.json`, `relais.json`, `pocsag.json` | Übersichten der Decoder |
| `/digi/freedv.json` | FreeDV: Codec-2-Zustand, DV-Decoder und gehörte Rufzeichen (`text`-Treffer von `rade`/`freedv` aus dem Ereignisspeicher) |
| `/logbuch/api/chat`, `/logbuch/api/online` | Chat, Hörer online (Pfadname aus alter Zeit) |
| `/admin/`, `/api/admin/…` | Admin-Seite und ihre Schnittstelle (nur mit Admin-Anmeldung) |
| `/api/auth/…` | Anmelden, Passwort ändern, eigenes Konto |
| `/api/directory` | Angaben fürs Verzeichnis (404, solange `[directory]` aus ist) |

## Oberfläche (`web/`)

Reines HTML/CSS/JavaScript ohne Build-Schritt.

| Datei | Aufgabe |
|---|---|
| `index.html` | Hören-Seite: Bandleiste, Wasserfälle, Bedienfeld, Chat |
| `core.js` | Kern: WebSockets, Wasserfall-Zeichnen, Ton (Opus über WebCodecs, AudioWorklet), Abstimmen, Zoom, Rauschsperre |
| `ui.js` | Bedienung: Bandleiste, Schnellwahl, Frequenzliste, S-Meter, Tastatur, Teilen, Aufnahme, Chat |
| `ui.css`, `look.css` | Aussehen (hell/dunkel) |
| `digi/`, `info/` | Unterseiten; `common.js` setzt Kopf, Reiter und Stationsname aus `ui.json` |
| `account.js` | Anmelden auf der Hören-Seite (Mitglieder-Bänder) |
| `admin/` | Admin-Seite (kein Inline-Code, eigene Sicherheitsköpfe) |

## Decoder-Plugins (`plugins/<name>/`)

`decoder.json` beschreibt, was das Plugin braucht; das Programm liest Ton von stdin und schreibt Meldungen als
JSON-Zeilen auf stdout. Der Server kümmert sich um Start, Neustart, Pegel und Weitergabe.

Liefert ein Plugin Ton zurück (`"output": {"kind": "audio", "rate": 8000}`, z. B. FreeDV), ist stdout rohes s16le mono
mit dieser Rate und die Meldungen kommen als JSON-Zeilen auf stderr (Zeilen, die mit `{` beginnen). Der Server rechnet
den Ton auf 24 kHz um, kodiert ihn als Opus und gibt ihn an Hörer weiter, die per `listen_decoder` statt eines Kanals den
Decoder gewählt haben, sowie unter `/stream/decoder/<id>.ogg`. Kodiert wird nur, solange jemand zuhört.

**Audio-Plugins mitgeliefert:** `freedv` (Python, libcodec2 per ctypes: Codec-2-Modes 700D/700E/1600, Ton 8 kHz) und
`rade` (C-Programm `crab_rade` gegen rade_c: RADE V1, die Wellenform der FreeDV-App ab 2.0, Ton 16 kHz). Beide hören
USB mit 8 kHz, 3 kHz breit, mit Pegelregler (`level: norm`), und melden `{"kind":"sync","mode":…,"on":…,"snr":…}` beim
Kommen und Gehen des Signals (entprellt: `snr_min`, `hold`), `freedv` außerdem `text` aus dem Textkanal. `rade` meldet im
`sync` zusätzlich `foff` (Frequenzversatz des Signals in Hz, aus `rade_freq_offset`), am Ende einer Durchgabe ein `eoo` und,
wenn der EOO-Rahmen ein gültiges Rufzeichen trägt (Format der FreeDV-App: LDPC HRA_56_56 + CRC-8, Modul
`plugins/rade/eoo/EooCallsignCodec` aus peterbmarks/radae_decoder, LGPL 2.1, per C-Brücke `eoo_bridge.h`), ein `text` mit
dem Rufzeichen. Die Sprachsynthese (FARGAN) beginnt bei jedem neuen Sync von vorn. `crab_rade` wird aus
`plugins/rade/crab_rade.c` gebaut (`plugins/rade/build.sh`, C und C++; statisch für Linux `packaging/build-rade.sh`) und
enthält die Modellgewichte; das Programm liegt nicht im Repo. Testsender `rade_eoo_tx` (`EOO_TX=1 build.sh`): Sprache
16 kHz → RADE-Signal mit EOO-Rufzeichen, zum Prüfen des Decoders ohne Funkgerät.

**Betriebsart DV** (`DecoderHub::ensure_dv`): auf der Hörerfrequenz (100-Hz-Raster) entsteht je lauffähigem DV-Plugin
(`DV_PLUGINS` = `rade`, `freedv`; lauffähig = decoder.json mit `output` und alle `requires` vorhanden) ein flüchtiger
Decoder; zusammen bilden sie die Gruppe `dv-<Hz>`, die der Hörer abonniert (`{"type":"dv","freq":…}` →
`{"type":"dv","id":"dv-<Hz>"}`). Von der Gruppe spielt nur das Mitglied mit Lock (`pick_active`: bisheriges bleibt,
solange es Lock hat, sonst das erste in `DV_PLUGINS`); Sync-Meldungen an die Hörer des Bandes tragen die Gruppen-ID und
den Lock der Gruppe (`data.on`), dazu `data.plugin`; `text`-Treffer (Rufzeichen) ebenso unter der Gruppen-ID. Die
Oberfläche zeigt das jüngste Rufzeichen als Marke neben dem Knopf „DV“ (`_crabDecTextMark` in `core.js`: gehörte Gruppe,
sonst DV-Gruppe nahe der eingestellten Frequenz; Anfangszustand aus `/api/decoders/events`, dort noch unter der
Mitglieds-ID `dv-<Hz>-rade`). `dv_max` begrenzt die Gruppen (Frequenzen), nicht die Prozesse; ohne Hörer enden alle
Mitglieder (sofort beim Weiterdrehen, sonst nach ≈ 90 s); ihre Treffer bleiben im Ringpuffer und erscheinen auf der
Digital-Seite im Reiter „FreeDV“.

```json
{
  "name": "aprs",
  "label": "APRS",
  "input": { "kind": "audio", "mode": "fm", "rate": 48000, "bandwidth": 12500, "level": "raw", "gain": 10 },
  "command": ["python3", "aprs.py"],
  "requires": ["direwolf", "python3"]
}
```

Umgebung: `CRAB_FREQ`, `CRAB_RATE`, `CRAB_MODE`, `CRAB_BAND`, `CRAB_ID`, `CRAB_DATA` (eigener Datenordner),
`CRAB_STATION_*`, Optionen als `CRAB_OPT_<NAME>`. Eine Meldung: `{"kind": "position", "src": "…", …}`; was ein Plugin
für die Digital-Seite ablegt (z. B. `aprs.json`), liegt in seinem Datenordner.
