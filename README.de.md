<p align="center">
  <img src="docs/crabsdr-logo-horizontal.svg" alt="crabSDR" width="400" />
</p>

<p align="center">
  <b>Modernes, minimalistisches WebSDR.</b> Ziel: stabil und einfach.
</p>

<p align="center">
  <a href="#installieren">Installieren</a> ·
  <a href="docs/CONFIG.md">Konfiguration</a> ·
  <a href="docs/STREAM.md">Stream</a> ·
  <a href="docs/ARCHITECTURE.md">Aufbau</a> ·
  <a href="CHANGELOG.md">Änderungen</a> ·
  <a href="https://crabsdr.de">crabsdr.de</a> ·
  <a href="README.md">English</a>
</p>

![Hören: Wasserfall, Abstimmen, S-Meter](docs/screenshots/hoeren.png)

- Viele Hörer an einem Empfänger, jeder auf seiner Frequenz (FM, AM, USB, LSB, CW).
- Mehrere Bänder je Station: RTL-SDR, `rtl_tcp`, HackRF, SoapySDR (`rx_sdr`).
- Decoder-Plugins, die immer mithören: APRS (mit RX-iGate), FT8, SSTV.
- Chat, Logbuch, Admin-Seite für Bänder, Decoder, Benutzer und die Konfigurationsdatei.
- Fertige Programme für x86-64, ARM64 und ARMv7 (Raspberry Pi ab Pi 2).

| Zwei Bänder | Digital: APRS | Handy |
|---|---|---|
| ![Zwei Bänder](docs/screenshots/zwei-baender.png) | ![APRS-Karte](docs/screenshots/digital-aprs.png) | ![Handy](docs/screenshots/handy.png) |

## Installieren

Downloads: [Releases](https://github.com/do1xx/crabSDR/releases).

**Raspberry Pi OS, Debian, Ubuntu** – Debian-Paket (`arm64`: Pi 3/4/5 mit 64-Bit-System, `armhf`: 32-Bit-System ab
Pi 2, `amd64`: PC):

```bash
sudo apt install ./crabsdr_1.1.0_arm64.deb
```

Danach `http://<rechner>:8080` öffnen. Die Admin-Seite liegt hinter der Krabbe unten rechts; das erste Passwort für
das Konto `admin` steht im Protokoll (`journalctl -u crabsdr | grep Passwort`) und muss beim ersten Anmelden geändert
werden. Konfiguration: `/etc/crabsdr/config.toml` (oder auf der Admin-Seite). Updates überschreiben sie nie.

**Andere Linux-Systeme** – Release-Archiv:

```bash
tar xzf crabsdr-1.1.0.tar.gz && cd crabsdr-1.1.0 && sudo ./install.sh
```

**Docker** – im Archiv liegt eine `docker-compose.yml`: `docker compose up -d`.

**Aus dem Quellcode** – Rust stable:

```bash
cd backend-rs && cargo build --release
./target/release/crabsdr-server ../packaging/config.example.toml
```

## Konfiguration

Eine TOML-Datei; eine Station braucht einen Namen und ein Band.

```toml
[station]
name = "Mein WebSDR"
locator = "JO53RB"

[[bands]]
id = "2m"
label = "2 m"
driver = "rtl_sdr"          # rtl_sdr, rtl_tcp, hackrf, rx_sdr
device = "0"                # Index oder Seriennummer
center_freq = 145000000
gain = 30

[[decoders]]                # optional
plugin = "aprs"
freq = 144800000
```

Vor dem Neustart prüfen: `crabsdr-server --check /etc/crabsdr/config.toml`. Alle Einstellungen:
[docs/CONFIG.md](docs/CONFIG.md), kommentierte Vorlage: [packaging/config.example.toml](packaging/config.example.toml).

## Decoder

| Plugin | Braucht | Ergebnis |
|---|---|---|
| `aprs` | direwolf | Pakete, Stationen, optional RX-iGate zu APRS-IS |
| `ft8` | jt9 (WSJT-X) | Dekodierungen, Stationen über 90 Tage |
| `sstv` | python3-numpy, python3-pil | Bilder (Martin, Scottie, Robot, PD) |

Ein Decoder ist ein Ordner in `plugins/` mit einer `decoder.json` und einem Programm, das Ton von stdin liest und
JSON-Zeilen ausgibt ([docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)).

## Entwicklung

```bash
(cd backend-rs && cargo test --workspace)
tools/e2e/run.sh            # Browsertests, siehe tools/e2e/README.md
```

Die Weboberfläche in `web/` ist reines HTML, CSS und JavaScript ohne Build-Schritt.

## Lizenz

GNU AGPL 3.0 oder neuer – siehe [LICENSE](LICENSE), Drittcode in [THIRD-PARTY.md](THIRD-PARTY.md). Copyright 2026 Dirk, DO1XX.
