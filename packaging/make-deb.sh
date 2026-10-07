#!/bin/sh
# make-deb.sh — Debian-Pakete (Raspberry Pi OS, Debian, Ubuntu) aus den fertigen Binaries bauen:
#   packaging/make-deb.sh [version] [amd64 arm64 armhf]   →  dist/crabsdr_<version>_<arch>.deb
# Braucht dpkg-deb (Debian/Ubuntu, GitHub-Runner) und dist/bin/<amd64|arm64|armv7>/crabsdr-server (build-binaries.sh).
# armhf nutzt das ARMv7-Binary: Pi 2 und neuer mit 32-Bit-System (nicht Pi Zero/1, die sind ARMv6).
# Aufbau wie install.sh: /opt/crabsdr/{bin,web,plugins}, Dienst crabsdr (Benutzer crabsdr), /etc/crabsdr/config.toml
# (wird nur beim ersten Mal aus /usr/share/crabsdr/config.example.toml angelegt – Änderungen über die Admin-Seite
# fasst ein Update nie an), Daten /var/lib/crabsdr (bleiben auch beim Entfernen).
set -e
cd "$(dirname "$0")/.."
# Version: Argument, sonst aus Cargo.toml + Git-Stand (z. B. 0.2.0+git20260929.3f7a06a, sortiert nach 0.2.0)
V=${1:-$(sed -n 's/^version = "\(.*\)"/\1/p' backend-rs/Cargo.toml | head -1)+git$(date -u +%Y%m%d).$(git rev-parse --short HEAD 2>/dev/null || echo lokal)}
shift 2>/dev/null || true
ARCHS=${*:-"amd64 arm64 armhf"}
for a in $ARCHS; do
  case $a in amd64) B=amd64 ;; arm64) B=arm64 ;; armhf) B=armv7 ;; *) echo "unbekannte Architektur $a"; exit 1 ;; esac
  [ -f dist/bin/$B/crabsdr-server ] || { echo "dist/bin/$B/crabsdr-server fehlt (packaging/build-binaries.sh $B)"; exit 1; }
  R=dist/deb-$a; rm -rf "$R"
  install -d "$R/DEBIAN" "$R/opt/crabsdr/bin" "$R/usr/bin" "$R/usr/lib/systemd/system" "$R/usr/share/crabsdr" "$R/usr/share/doc/crabsdr"
  install -m 755 dist/bin/$B/crabsdr-server "$R/opt/crabsdr/bin/crabsdr-server"
  ln -s /opt/crabsdr/bin/crabsdr-server "$R/usr/bin/crabsdr-server"
  cp -r web "$R/opt/crabsdr/web"
  install -d "$R/opt/crabsdr/plugins"; for p in aprs ft8 sstv; do cp -r plugins/$p "$R/opt/crabsdr/plugins/"; done
  find "$R/opt/crabsdr" -name __pycache__ -prune -exec rm -rf {} + ; find "$R/opt/crabsdr" -name .DS_Store -delete
  install -m 644 packaging/crabsdr.service "$R/usr/lib/systemd/system/crabsdr.service"
  install -m 644 packaging/config.example.toml "$R/usr/share/crabsdr/config.example.toml"
  install -m 644 LICENSE "$R/usr/share/doc/crabsdr/copyright"
  install -m 644 docs/CONFIG.md CHANGELOG.md "$R/usr/share/doc/crabsdr/"
  SIZE=$(du -sk "$R" | cut -f1)
  cat > "$R/DEBIAN/control" <<CTL
Package: crabsdr
Version: $V
Section: hamradio
Priority: optional
Architecture: $a
Maintainer: DO1XX <do1xx@pm.me>
Installed-Size: $SIZE
Depends: adduser, rtl-sdr
Recommends: python3, python3-numpy, python3-pil, direwolf, multimon-ng, libcodec2-1.2
Suggests: wsjtx
Homepage: https://crabsdr.de
Description: modernes, minimalistisches WebSDR
 crabSDR zeigt einen oder mehrere SDR-Empfänger (RTL-SDR, rtl_tcp, HackRF,
 SoapySDR) im Browser: Wasserfall, Ton, Decoder für APRS, FT8 und SSTV,
 Chat, Logbuch und eine Admin-Seite. Nach der Installation: http://<rechner>:8080
CTL
  cat > "$R/DEBIAN/postinst" <<'SH'
#!/bin/sh
set -e
[ "$1" = configure ] || exit 0
id crabsdr >/dev/null 2>&1 || adduser --system --group --home /var/lib/crabsdr --no-create-home --quiet crabsdr
adduser --quiet crabsdr plugdev >/dev/null 2>&1 || true
install -d -o crabsdr -g crabsdr /var/lib/crabsdr
install -d -o root -g crabsdr -m 2770 /etc/crabsdr
if [ ! -f /etc/crabsdr/config.toml ]; then
  install -m 660 -o root -g crabsdr /usr/share/crabsdr/config.example.toml /etc/crabsdr/config.toml
  echo "crabSDR: Konfiguration angelegt: /etc/crabsdr/config.toml (Station, Frequenz, Gain anpassen)"
fi
if [ -d /run/systemd/system ]; then
  systemctl daemon-reload
  systemctl enable crabsdr >/dev/null 2>&1 || true
  if runuser -u crabsdr -- /opt/crabsdr/bin/crabsdr-server --check /etc/crabsdr/config.toml >/dev/null 2>&1; then
    systemctl restart crabsdr
    P=$(sed -n 's/^port *= *\([0-9]*\).*/\1/p' /etc/crabsdr/config.toml | head -1)
    echo "crabSDR läuft: http://$(hostname -I 2>/dev/null | cut -d' ' -f1):${P:-8080}  (Admin-Seite: Krabbe unten rechts)"
    sleep 3; journalctl -u crabsdr -n 300 --no-pager 2>/dev/null | grep -o "Admin-Konto „admin“, Passwort: [A-Za-z0-9]*" | tail -1 || true
  else
    echo "crabSDR: Konfiguration fehlerhaft – prüfen: crabsdr-server --check /etc/crabsdr/config.toml, dann systemctl restart crabsdr"
  fi
fi
exit 0
SH
  cat > "$R/DEBIAN/prerm" <<'SH'
#!/bin/sh
set -e
if [ "$1" = remove ] && [ -d /run/systemd/system ]; then systemctl stop crabsdr 2>/dev/null || true; systemctl disable crabsdr >/dev/null 2>&1 || true; fi
exit 0
SH
  cat > "$R/DEBIAN/postrm" <<'SH'
#!/bin/sh
set -e
[ -d /run/systemd/system ] && systemctl daemon-reload 2>/dev/null || true
if [ "$1" = purge ]; then
  rm -rf /etc/crabsdr
  echo "crabSDR: Konfiguration entfernt. Daten (Benutzer, Chat, Logbuch, Decoder) bleiben in /var/lib/crabsdr."
fi
exit 0
SH
  chmod 755 "$R/DEBIAN/postinst" "$R/DEBIAN/prerm" "$R/DEBIAN/postrm"
  dpkg-deb --root-owner-group --build "$R" "dist/crabsdr_${V}_$a.deb" >/dev/null
  rm -rf "$R"; ls -la "dist/crabsdr_${V}_$a.deb"
done
