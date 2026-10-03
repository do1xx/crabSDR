<p align="center">
  <img src="docs/crabsdr-logo-horizontal.svg" alt="crabSDR" width="400" />
</p>

<p align="center">
  <b>A modern, minimalist WebSDR.</b> Goal: stable and simple.
</p>

<p align="center">
  <a href="#install">Install</a> ·
  <a href="docs/CONFIG.md">Configuration</a> ·
  <a href="docs/ARCHITECTURE.md">Architecture</a> ·
  <a href="CHANGELOG.md">Changes</a> ·
  <a href="https://crabsdr.de">crabsdr.de</a> ·
  <a href="README.de.md">Deutsch</a>
</p>

![Listening: waterfall, tuning, S-meter](docs/screenshots/hoeren.png)

- Many listeners on one receiver, each on their own frequency (FM, AM, USB, LSB, CW).
- Several bands per station: RTL-SDR, `rtl_tcp`, HackRF, SoapySDR (`rx_sdr`).
- Decoder plugins that always listen: APRS (with RX iGate), FT8, SSTV.
- Chat, logbook, admin page for bands, decoders, users and the config file.
- Static binaries for x86-64, ARM64 and ARMv7 (Raspberry Pi 2 and newer).

| Two bands | Digital: APRS | Phone |
|---|---|---|
| ![Two bands](docs/screenshots/zwei-baender.png) | ![APRS map](docs/screenshots/digital-aprs.png) | ![Phone](docs/screenshots/handy.png) |

## Install

Downloads: [Releases](https://github.com/do1xx/crabSDR/releases).

**Raspberry Pi OS, Debian, Ubuntu** – Debian package (`arm64`: Pi 3/4/5 with 64-bit OS, `armhf`: 32-bit OS on Pi 2 and
newer, `amd64`: PC):

```bash
sudo apt install ./crabsdr_0.4.3_arm64.deb
```

Then open `http://<machine>:8080`. The admin page is behind the crab in the bottom right corner; the first password
for the account `admin` is in the log (`journalctl -u crabsdr | grep Passwort`) and must be changed on first sign-in.
Configuration: `/etc/crabsdr/config.toml` (or on the admin page). Updates never overwrite it.

**Other Linux** – release archive:

```bash
tar xzf crabsdr-0.4.3.tar.gz && cd crabsdr-0.4.3 && sudo ./install.sh
```

**Docker** – the archive contains `docker-compose.yml`: `docker compose up -d`.

**From source** – Rust stable:

```bash
cd backend-rs && cargo build --release
./target/release/crabsdr-server ../packaging/config.example.toml
```

## Configuration

One TOML file; a station needs a name and one band.

```toml
[station]
name = "My WebSDR"
locator = "JO53RB"

[[bands]]
id = "2m"
label = "2 m"
driver = "rtl_sdr"          # rtl_sdr, rtl_tcp, hackrf, rx_sdr
device = "0"                # index or serial number
center_freq = 145000000
gain = 30

[[decoders]]                # optional
plugin = "aprs"
freq = 144800000
```

Check before restarting: `crabsdr-server --check /etc/crabsdr/config.toml`. All keys: [docs/CONFIG.md](docs/CONFIG.md)
(German), commented template: [packaging/config.example.toml](packaging/config.example.toml).

## Decoders

| Plugin | Needs | Result |
|---|---|---|
| `aprs` | direwolf | packets, stations, optional RX iGate to APRS-IS |
| `ft8` | jt9 (WSJT-X) | decodes, stations over 90 days |
| `sstv` | python3-numpy, python3-pil | images (Martin, Scottie, Robot, PD) |

A decoder is a folder in `plugins/` with a `decoder.json` and a program that reads audio on stdin and prints JSON
lines ([docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)).

## Development

```bash
(cd backend-rs && cargo test --workspace)
tools/e2e/run.sh            # browser tests, see tools/e2e/README.md
```

The web interface in `web/` is plain HTML, CSS and JavaScript without a build step. Code comments and most
documentation are in German.

## License

MIT – see [LICENSE](LICENSE). Dirk, DO1XX.
