# Änderungen

Versionen nach [Semantic Versioning](https://semver.org/lang/de/). `crabsdr-server --version` zeigt Version und Git-Stand.

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
