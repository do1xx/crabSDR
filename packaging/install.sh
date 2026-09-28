#!/bin/sh
# install.sh — crabSDR ohne Docker als systemd-Dienst einrichten (Debian/Ubuntu/Raspberry Pi OS/Armbian).
# Im entpackten Release-Ordner als root:  sudo ./install.sh     →  http://<rechner>:8080
set -e
cd "$(dirname "$0")"
case "$(uname -m)" in x86_64) A=amd64 ;; aarch64|arm64) A=arm64 ;; armv7l|armv7*) A=armv7 ;; *) echo "Architektur $(uname -m) nicht unterstützt"; exit 1 ;; esac
[ -f "bin/$A/crabsdr-server" ] || { echo "bin/$A/crabsdr-server fehlt"; exit 1; }
echo "crabSDR für $A installieren …"
command -v rtl_sdr >/dev/null || { apt-get update -qq && apt-get install -y -qq rtl-sdr; }
# DVB-Treiber sperren, sonst gehört der Stick dem Kernel
if ! grep -qs dvb_usb_rtl28xxu /etc/modprobe.d/*.conf; then echo 'blacklist dvb_usb_rtl28xxu' > /etc/modprobe.d/rtl-sdr-blacklist.conf; modprobe -r dvb_usb_rtl28xxu 2>/dev/null || true; fi
id crabsdr >/dev/null 2>&1 || useradd --system --home /var/lib/crabsdr --shell /usr/sbin/nologin --groups plugdev crabsdr
install -d -o crabsdr -g crabsdr /var/lib/crabsdr
install -d /opt/crabsdr/bin /opt/crabsdr/plugins
# Konfiguration: Gruppe crabsdr darf schreiben (Admin-Seite); neue Dateien erben die Gruppe
install -d -o root -g crabsdr -m 2770 /etc/crabsdr
install -m 755 "bin/$A/crabsdr-server" /opt/crabsdr/bin/crabsdr-server
rm -rf /opt/crabsdr/web && cp -r web /opt/crabsdr/web
# Decoder-Plugins (APRS, FT8, SSTV …); eingeschaltet werden sie mit [[decoders]] in /etc/crabsdr/config.toml
if [ -d plugins ]; then rm -rf /opt/crabsdr/plugins && cp -r plugins /opt/crabsdr/plugins; fi
if [ ! -f /etc/crabsdr/config.toml ]; then
  install -m 660 -g crabsdr config.example.toml /etc/crabsdr/config.toml
  echo "Konfiguration angelegt: /etc/crabsdr/config.toml (Station, Frequenz, Gain anpassen; Erklärung: docs/CONFIG.md)"
fi
install -m 644 crabsdr.service /etc/systemd/system/crabsdr.service
# vor dem (Neu-)Start prüfen, als Dienstbenutzer: eine fehlerhafte Konfiguration hält den laufenden Dienst nicht an
if ! runuser -u crabsdr -- /opt/crabsdr/bin/crabsdr-server --check /etc/crabsdr/config.toml; then
  echo "Konfiguration fehlerhaft – bitte /etc/crabsdr/config.toml korrigieren, dann: systemctl restart crabsdr"; exit 1
fi
systemctl daemon-reload && systemctl enable crabsdr >/dev/null 2>&1 && systemctl restart crabsdr
P=$(sed -n 's/^port *= *\([0-9]*\).*/\1/p' /etc/crabsdr/config.toml | head -1)
sleep 3; systemctl is-active crabsdr && echo "läuft: http://$(hostname -I 2>/dev/null | cut -d' ' -f1):${P:-8080}   Admin-Seite: /admin/"
journalctl -u crabsdr -n 200 --no-pager 2>/dev/null | grep -o "Admin-Konto „admin“, Passwort: [A-Za-z0-9]*" | tail -1
echo "Nach Änderungen: crabsdr-server --check /etc/crabsdr/config.toml && systemctl restart crabsdr"
