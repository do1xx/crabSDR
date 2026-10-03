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
| `crabsdr-server` | Webserver (axum): WebSocket je Band, DSP-Thread, Hörerverwaltung, Decoder, Chat/Logbuch, Seiten-Daten, `--check` |

### Wichtige Dateien im Server

| Datei | Inhalt |
|---|---|
| `main.rs` | Start, Kommandozeile, Routen, WebSocket-Befehle, `ui.json`/`bandinfo.js` für die Oberfläche |
| `sdr_pipeline.rs`, `sdr_manager.rs` | eine Pipeline je Band: Quelle, DSP-Thread, Hörer, Zustand |
| `dsp_thread.rs` | FFT, Wasserfall (inkl. 2 min Verlauf als JPEG beim Anmelden), Kanäle gruppiert nach Frequenz/Modus/Bandbreite |
| `client.rs` | Zustand je Hörer; Hörer mit gleichem `ChannelKey` teilen Filter, Demodulator und Opus-Encoder |
| `decoders.rs` | `[[decoders]]`: Plugin starten, Rohton zuführen, Meldungen (JSON-Zeilen) sammeln, Neustart mit Pause, MQTT |
| `chat.rs` | Chat und Logbuch (SQLite `pinnwand.db`), Hörer online |
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
| `/`, `/digi/`, `/logbuch/`, `/info/` | Oberfläche (`web/`, Stationsordner hat Vorrang) |
| `/bandinfo.js`, `/ui.json` | Bänder und Einstellungen für die Oberfläche |
| `/api/health` | Zustand, Version, Laufzeit |
| `/api/bands`, `/api/listeners` | Bänder, Hörer (`n` Personen, `n_audio` hören gerade, `connections` Verbindungen) |
| `/api/decoders`, `/api/decoders/events?since=&wait=1` | Decoder-Zustand, Meldungen (Langabfrage) |
| `/digi/aprs.json`, `ft8.json`, `sstv.json`, `relais.json` | Übersichten der Decoder |
| `/logbuch/api/chat`, `/logbuch/api/log`, `/logbuch/api/online` | Chat, Logbuch, Hörer online |
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
| `digi/`, `logbuch/`, `info/` | Unterseiten; `common.js` setzt Kopf, Reiter und Stationsname aus `ui.json` |
| `account.js` | Anmelden auf der Hören-Seite (Mitglieder-Bänder) |
| `admin/` | Admin-Seite (kein Inline-Code, eigene Sicherheitsköpfe) |

## Decoder-Plugins (`plugins/<name>/`)

`decoder.json` beschreibt, was das Plugin braucht; das Programm liest Ton von stdin und schreibt Meldungen als
JSON-Zeilen auf stdout. Der Server kümmert sich um Start, Neustart, Pegel und Weitergabe.

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
