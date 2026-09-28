<p align="center">
  <img src="docs/crabsdr-logo-horizontal.svg" alt="crabSDR" width="400" />
</p>

<p align="center">
  <b>Ein WebSDR zum Selbstbetreiben.</b> Ein kleines Programm macht aus RTL-SDR-Sticks einen Empfänger, den viele
  Leute gleichzeitig im Browser nutzen – jeder auf seiner eigenen Frequenz.
</p>

<p align="center">
  <a href="#schnellstart">Schnellstart</a> ·
  <a href="docs/CONFIG.md">Konfiguration</a> ·
  <a href="docs/ARCHITECTURE.md">Aufbau</a> ·
  <a href="CHANGELOG.md">Änderungen</a> ·
  <a href="README.md">English</a>
</p>

![Hören: Wasserfall, Abstimmen, S-Meter](docs/screenshots/hoeren.png)

## Was es kann

- **Viele Hörer, ein Empfänger.** Jeder stimmt selbst ab (FM, AM, USB, LSB, CW). Hörer auf derselben Frequenz
  teilen sich einen Kanal – hundert Hörer kosten kaum mehr als einer.
- **Mehrere Bänder nebeneinander.** Ein Stick je Band; Bänder ein- und ausschalten, sie teilen sich den Bildschirm.
- **Sauberer Ton.** FFT-Kanalfilter mit durchgehender Phase, FM ohne Pumpen, Opus mit 24 kHz (sonst PCM).
- **Decoder, die immer mithören.** APRS (mit RX-iGate zu APRS-IS), FT8 und SSTV laufen dauerhaft als Plugins.
  Ergebnisse auf der Seite **Digital**: Karte, Stationen, Pakete, Reichweite, SSTV-Galerie. Optional per MQTT.
- **Für die Hörer:** Chat, Logbuch („wen hast du von wo gehört“) mit Entfernung und Richtung, Teilen-Links mit
  Frequenz und Betriebsart, Aufnahme als WAV, Tastaturkürzel, hell und dunkel, Handy-Ansicht.
- **Für den Betreiber:** Admin-Seite im Browser (`/admin/`) für Bänder, Decoder, Benutzer und die Konfiguration,
  eine kleine Konfigurationsdatei mit sinnvollen Voreinstellungen, `crabsdr-server --check` vor jedem Neustart,
  statische Programme für x86-64, ARM64 und ARMv7 (Raspberry Pi), Docker, systemd-Dienst.
- **Mitglieder und eigene Bänder.** Bänder können öffentlich, für angemeldete Mitglieder oder nur für Admins sein;
  Decoder genauso. Konten, Sitzungen und Admin-Seite folgen [docs/SECURITY.md](docs/SECURITY.md).

| Zwei Bänder | Digital: APRS | Handy |
|---|---|---|
| ![Zwei Bänder](docs/screenshots/zwei-baender.png) | ![APRS-Karte](docs/screenshots/digital-aprs.png) | ![Handy](docs/screenshots/handy.png) |


## Schnellstart

Gebraucht wird ein RTL-SDR-Stick, oder eine Quelle, die `rtl_tcp` spricht, ein HackRF oder SoapySDR über `rx_sdr`.

### Release-Archiv (Debian, Ubuntu, Raspberry Pi OS)

```bash
tar xzf crabsdr-0.2.0.tar.gz && cd crabsdr-0.2.0
sudo ./install.sh
```

`install.sh` installiert `rtl-sdr`, sperrt den DVB-Treiber des Kernels, legt den Dienstbenutzer an, kopiert alles
nach `/opt/crabsdr` und eine Startkonfiguration nach `/etc/crabsdr/config.toml`, prüft sie und startet den Dienst.
Dann `http://<rechner>:8080` öffnen. Die Admin-Seite ist `/admin/`; das erste Passwort des Kontos `admin` steht einmal im
Protokoll (`journalctl -u crabsdr | grep Passwort`) und muss beim ersten Anmelden geändert werden. Nach Änderungen
von Hand:

```bash
sudo /opt/crabsdr/bin/crabsdr-server --check /etc/crabsdr/config.toml && sudo systemctl restart crabsdr
```

### Docker

```bash
sh packaging/build-binaries.sh amd64      # oder arm64 / armv7; braucht Docker, baut statische Programme nach dist/bin
docker compose -f packaging/docker-compose.yml up -d --build
```

Der erste Start legt `data/config.toml` (neben der compose-Datei) aus der Vorlage an. Das volle Image enthält die
Decoder (direwolf, WSJT-X, numpy); `packaging/Dockerfile.lite` baut ein kleines Image ohne sie.

### Aus den Quellen

```bash
cd backend-rs && cargo build --release
./target/release/crabsdr-server ../packaging/config.example.toml
```

## Konfiguration

Eine TOML-Datei. Eine Station braucht einen Namen und ein Band, alles andere hat Voreinstellungen.

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

Jede Einstellung erklärt [docs/CONFIG.md](docs/CONFIG.md), eine kommentierte Vorlage ist
[packaging/config.example.toml](packaging/config.example.toml). Eine fehlerhafte oder unlesbare Konfiguration startet
nie still mit Voreinstellungen: crabSDR hält mit Zeile und Spalte des Fehlers an, und `--check` meldet Tippfehler,
Decoder außerhalb der Bänder, fehlende Programme und nicht erreichbare Quellen.

Ein **Stationsordner** (`site_dir`) ergänzt eigenes Logo, Schnellwahl, Marker, Relaisliste oder eine eigene
Info-Seite, ohne die Programmdateien anzufassen.

## Decoder

Ein Decoder ist ein Ordner in `plugins/` mit einer `decoder.json` (was er braucht: Betriebsart, Abtastrate,
Bandbreite, Programme) und einem Programm, das Ton von stdin liest und JSON-Zeilen ausgibt. crabSDR startet es,
versorgt es, startet es bei Bedarf neu und gibt die Ergebnisse weiter. Dabei sind:

| Plugin | braucht | Ergebnis |
|---|---|---|
| `aprs` | direwolf | Pakete, Stationen (direkt oder über Digipeater), Relais, optional RX-iGate |
| `ft8` | jt9 (WSJT-X) | Decodes, Stationen mit Locator über 90 Tage |
| `sstv` | python3-numpy, python3-pil | Bilder (Martin, Scottie, Robot, PD – die ISS sendet PD 120) |

## Entwicklung

```bash
python3 tools/phase0/synth.py testdata/synth.cu8 --sec 30   # Testsignal für den Golden-Test der DSP
(cd backend-rs && cargo test --workspace)
tools/e2e/run.sh                                            # Browser-Tests, siehe tools/e2e/README.md
```

Die Oberfläche in `web/` ist reines HTML, CSS und JavaScript ohne Build-Schritt.

## Lizenz

MIT – siehe [LICENSE](LICENSE). crabSDR enthält keinen Code aus anderen WebSDR-Programmen.

## Autor

Dirk, DO1XX
