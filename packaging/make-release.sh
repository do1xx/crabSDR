#!/bin/sh
# make-release.sh — Release-Archiv bauen: dist/crabsdr-<version>.tar.gz mit bin/{amd64,arm64,armv7}, web/, install.sh,
# crabsdr.service, config.example.toml, docker-compose.yml, Dockerfile, LICENSE. Vorher packaging/build-binaries.sh.
set -e
cd "$(dirname "$0")/.."
V=${1:-$(git describe --tags --always 2>/dev/null || date +%Y%m%d)}
R=dist/crabsdr-$V; rm -rf "$R"; mkdir -p "$R"
cp -r dist/bin "$R/bin"; cp -r web "$R/web"
mkdir -p "$R/plugins"; for p in aprs ft8 sstv; do cp -r plugins/$p "$R/plugins/"; done; find "$R/plugins" -name __pycache__ -prune -exec rm -rf {} +
cp packaging/install.sh packaging/crabsdr.service packaging/config.example.toml packaging/docker-entrypoint.sh packaging/soapymiri-transfer.patch LICENSE "$R/"
mkdir -p "$R/docs" && cp docs/CONFIG.md "$R/docs/"
# Dockerfile und compose mit den flachen Pfaden des Archivs
for f in Dockerfile Dockerfile.lite; do
  sed -e 's#dist/bin/#bin/#' -e 's#packaging/config.example.toml#config.example.toml#' -e 's#packaging/docker-entrypoint.sh#docker-entrypoint.sh#' -e 's#-f packaging/#-f #' packaging/$f > "$R/$f"
done
sed -e 's#build: { context: .., dockerfile: packaging/Dockerfile }#build: .#' packaging/docker-compose.yml > "$R/docker-compose.yml"
cat > "$R/LIESMICH.txt" <<'TXT'
crabSDR – freies WebSDR (MIT-Lizenz)

Mit Docker:     docker compose up -d        → http://<rechner>:8080   (Konfiguration: ./data/config.toml)
Ohne Docker:    sudo ./install.sh           → http://<rechner>:8080   (Konfiguration: /etc/crabsdr/config.toml)
Kleines Image (Alpine, nur RTL-SDR, ~20 MB):  docker build -f Dockerfile.lite -t crabsdr:lite .
Konfiguration prüfen:  crabsdr-server --check <config.toml>   (Erklärung aller Einstellungen: docs/CONFIG.md)
Admin-Passwort beim ersten Start im Log: journalctl -u crabsdr | grep Passwort (Docker: docker compose logs | grep Passwort).
Unterstützt: x86-64, ARM64 (Pi 4/5, Odroid N2), ARMv7 (Odroid XU4, Pi 2/3 32 Bit).
Empfänger: RTL-SDR (rtl_sdr), rtl_tcp übers Netz, HackRF (hackrf_transfer); Airspy, Mirics, LimeSDR usw. über
SoapySDR (rx_sdr) – im vollen Docker-Image enthalten, ohne Docker rx_tools selbst installieren.
Das Server-Programm ist statisch gebaut und braucht keine weiteren Bibliotheken.
TXT
# ohne macOS-Zusatzattribute (sonst warnt GNU tar beim Entpacken)
if tar --version 2>/dev/null | grep -q bsdtar; then X="--no-xattrs"; else X=""; fi
COPYFILE_DISABLE=1 tar $X -C dist -czf "dist/crabsdr-$V.tar.gz" "crabsdr-$V" && ls -la "dist/crabsdr-$V.tar.gz"
