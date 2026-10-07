# Änderungen

Versionen nach [Semantic Versioning](https://semver.org/lang/de/). `crabsdr-server --version` zeigt Version und Git-Stand.

## 1.2.0 – 2026-10-07

### Stream
- I/Q-Stream `…/stream/<kHz>/iq.wav` (links I, rechts Q, 48 oder 32 kHz, 16 oder 8 bit) und als Experiment `iq.ogg` (Opus auf I/Q): komplexes Basisband eines Kanals für externe Decoder (TETRA, DMR). Freigabe über `iq_stream` (Vorgabe nur Sysop), höchstens `max_iq` (1) gleichzeitig. Siehe docs/STREAM.md.

## 1.1.2 – 2026-10-07

### Oberfläche
- Info-Seite: Abschnitte „Link mit Einstellungen“ und „Stream für VLC, mpv und Decoder“ mit Anleitung und anklickbaren Beispielen aus der Schnellwahl der Station.

## 1.1.1 – 2026-10-07

### Stream
- Ogg/Opus-Stream trägt Titel (Frequenz, Betriebsart, Station) und Künstler in den OpusTags; liegt `logo.png` oder `logo.jpg` im Stationsordner, wird es als Titelbild mitgeschickt (VLC zeigt beides).

## 1.1.0 – 2026-10-07

Stream in WAV und Stereo, Link mit allen Einstellungen, Lastgrenzen.

### Stream
- `.wav` zusätzlich zu `.ogg`: PCM 16 bit 48 kHz, verlustfrei für Decoder (DMR, POCSAG, AFSK).
- `pair.wav?l=<kHz>/<mode>&r=<kHz>/<mode>`: zwei Kanäle auf links und rechts, für ein Audiokabel mit zwei Decodern.
- `br`: Opus-Bitrate je Stream (8–128 kbit/s); Opus wird jetzt je Stream kodiert, der DSP liefert PCM.

### Oberfläche
- Teilen-Link trägt Rauschsperre und AGC; im Link außerdem `vol`, `mute`, `name` und `ui=min` (Minimal-Ansicht ohne Wasserfall für viele Tabs). Tab-Titel zeigt Frequenz und Betriebsart.
- Hinweis statt stummem Fehlschlag, wenn die Station voll ist.

### Lastgrenzen
- Neue Schlüssel `max_listeners` (50), `max_per_ip` (10), `max_channels` (16), `max_streams` (6) je Band, 0 = keine Grenze. Jede andere Frequenz/Betriebsart/Bandbreite kostet einen DSP-Kanal, Hörer auf demselben Kanal teilen ihn; darüber „Station voll“.

## 1.0.0 – 2026-10-07

Erste Veröffentlichung.

- Ein Programm, eine Konfigurationsdatei: mehrere Empfänger (RTL-SDR, SDRplay über SoapySDR, rtl_tcp), jeder Hörer mit eigener Frequenz und Betriebsart (FM, AM, SAM, USB, LSB, CW, WFM, Daten).
- Wasserfall mit Zoom bis in den Kanal, Bereiche nach Bandplan, Schnellwahl, Teilen-Link, Aufnahme.
- Ton als Opus mit anpassendem Vorlauf; AGC wie am Transceiver mit Schwelle, Rauschsperre mit sofortigem Schließen.
- Ton-Stream für VLC, mpv und Decoder (`/stream/<kHz>/<betriebsart>.ogg`, Senderliste `presets.m3u`).
- Decoder als Plugins (APRS mit Karte und Digipeater-Übersicht, FT8, SSTV), Treffer per MQTT.
- Chat und Logbuch der Hörer, Verbund-Chat über crabsdr.de, Hörerzahl nach Personen.
- Admin-Seite: Bänder, Decoder, Betreiberangaben, Konfiguration ohne Neustart, Verzeichnis-Anmeldung bei crabsdr.de.
- Oberfläche für Handy und als Web-App, Tastaturbedienung, Info-Seite mit Impressum und Datenschutz.
- Debian-Pakete (amd64, arm64, armhf), statische Binärdateien und Docker-Image; Frequenzkorrektur je Band in Software.
