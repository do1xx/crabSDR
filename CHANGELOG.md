# Änderungen

Versionen nach [Semantic Versioning](https://semver.org/lang/de/). `crabsdr-server --version` zeigt Version und Git-Stand.

## Unveröffentlicht

### Empfänger
- SoapySDR-Geräte (`driver = "rx_sdr"`) werden mit 16 Bit gelesen statt 8 Bit – volle Dynamik für Airspy, SDRplay/MSi2500,
  LimeSDR, PlutoSDR; neue Band-Einstellung `format` (`cs16`, `cf32`, `cu8`); Gleichanteil wird abgezogen.
- MSi2500 / SDRplay RSP1 und Nachbauten über die freien Treiber libmirisdr-5 + SoapyMiri (`device = "driver=soapyMiri"`).
- Admin-Seite „Sticks“: zu jedem gefundenen Gerät der passende Eintrag fürs Band, Zuordnung auch für SoapySDR-Geräte.
- Hohe Abtastraten ohne Pfeifton: FFT-Größe automatisch nach Abtastrate (≈ 500-Hz-Bins), Kanal-Bins als Vielfaches
  von 4 (vorher fehlte je Block ein halbes Sample – auch bei 7-kHz-Filtern mit 2,048 MS/s).
- Protokoll: IQ-Blöcke nur beim Start und etwa stündlich.

## 0.2.0 – 2026-09-29

Eine einheitliche crabSDR-Fassung: gleiche Oberfläche für jede Station, alles über die Konfiguration einstellbar.

### Oberfläche
- Neue Oberfläche (`web/`): Bandleiste zum Ein-/Ausschalten, Bänder teilen sich den Bildschirm, Klick in den Wasserfall
  stimmt mit einstellbarem Raster ab (z. B. 5 oder 6,25 kHz), ruhiges zweizeiliges Bedienfeld.
- FM startet mit Rauschsperre; Betriebsart je Band aus der Konfiguration (erster Besuch des Bandes).
- Wasserfall sofort gefüllt (Server hält 2 min Verlauf je Band), Hörerliste in Echtzeit.
- Teilen-Link mit Frequenz und Betriebsart, Aufnahme mit Frequenz, Modulation und Uhrzeit im Dateinamen.
- Seiten **Digital** (APRS, FT8, SSTV, Reichweite auf der Karte), **Logbuch** (Hörer tragen ein, wen sie gehört haben,
  mit Entfernung und Richtung) und **Info** (baut sich aus Station, Bändern und Decodern).
- Chat und Logbuch dauerhaft (SQLite), „Hörer online“.
- Stationsordner (`site_dir`) für Logo, Schnellwahl, Marker, Relaisliste, eigene Seiten; Schalter in `[ui]`.
- Krabben-Logo und Favicon.

### Decoder
- Plugin-System `[[decoders]]`: Plugin + Frequenz, das Plugin beschreibt seinen Bedarf (`decoder.json`).
- Mitgeliefert: **APRS** (direwolf, mit RX-iGate zu APRS-IS, Passcode wird berechnet), **FT8** (jt9, Stationen über
  90 Tage), **SSTV** (eigener Decoder: Martin, Scottie, Robot, PD).
- Decoder-Menü in der Bedienleiste, API `/api/decoders`, Treffer optional per **MQTT**.
- SSTV: Sendetakt wird je Bild aus den Syncs gemessen und herausgerechnet, Sync-Kanten robust gegen Rauschen –
  keine Farbsäume mehr bei Sendern mit Taktfehler (geprüft bis 1 %), Bild sitzt pixelgenau (`tools/sstv/sstv_test.py`).

### Empfang und Klang
- Kanalfilter: Blockphase wird fortgesetzt – kein Pfeifton mehr auf Frequenzen außerhalb des 2-kHz-Rasters.
- FM ohne Lautstärkeregelung (kein Pumpen), AM/SSB-AGC mit Haltezeit, Opus 24 kHz / 32 kbit/s.
- S-Meter nach IARU Region 1 (S9 = −93 dBm), Kalibrierung je Band.

### Admin-Seite, Benutzer, Sicherheit
- Admin-Seite `/admin/`: Überblick, Station und Oberfläche, Bänder, Decoder, Benutzer, Chat und Logbuch, Konfiguration
  als Text (mit Prüfung, Sicherungen, Schutz gegen gleichzeitige Änderungen), Sticks und Seriennummern, Neustart.
- Benutzer mit Rolle Admin oder Nutzer; Bänder öffentlich, für Mitglieder oder nur Admin; nicht öffentliche Decoder
  nur für Zugeteilte. Anmelden auf der Hören-Seite.
- Anmeldung nach docs/SECURITY.md: Zufallspasswörter aus dem Betriebssystem, Passwortwechsel beim ersten Anmelden,
  Anmeldebremse, widerrufbare Sitzungen, getrennte Admin-Sitzung, strenge CSP, `crabsdr-server --reset-admin`.
- Behoben: Decoder-Dateischnittstelle lieferte auch Arbeitsdateien (z. B. direwolf.conf mit iGate-Passcode).
- Verzeichnis crabsdr.de (`[directory]`, standardmäßig aus): Station meldet alle 5 min ihre öffentlichen Angaben,
  das Verzeichnis prüft sie über `/api/directory` unter der öffentlichen Adresse. Schalter auf der Admin-Seite.

### Konfiguration und Betrieb
- Klarere Schlüssel: `[[bands]]` mit `driver`, `device`, `host`, `port`, `mode`, `smeter_cal`; alte Namen gelten weiter.
- Pfade werden gefunden, Voreinstellungen für fast alles – eine Station braucht Name und ein Band.
- `crabsdr-server --check`: prüft Syntax, Tippfehler, Bänder, Decoder, Plugins, Programme und Quellen.
- Fehlerhafte oder unlesbare Konfiguration beendet den Start mit Zeile und Spalte (vorher still Voreinstellungen).
- Statische Binaries (x86-64, ARM64, ARMv7), Docker voll/Lite, Release-Archiv mit `install.sh`, das vor dem Start prüft.
- Beschreibung aller Einstellungen: `docs/CONFIG.md`.
- Ruhiges Protokoll: fehlt eine Quelle dauerhaft, meldet der Treiber das einmal und danach höchstens alle 10 min
  (Abstand 2 → 30 s); ohne Terminal keine Farbcodes; Standard-Stufe `info`.
- Debian-Pakete für Raspberry Pi OS, Debian und Ubuntu (`arm64`, `armhf`, `amd64`); die Konfiguration fasst ein Update nie an.
- Automatischer Bau nach jeder Änderung auf `main` (Vorabversion „Entwicklungsstand“), Release-Entwurf bei jedem Tag `v*`.

## 0.1.0 – 2026-03

Erste Fassung: Rust-Kern mit FFT-Kanalfilter, mehrere Bänder, Svelte-Oberfläche, Benutzerverwaltung, Plugins der
ersten Generation (RDS, multimon-ng, rtl_433 …).
