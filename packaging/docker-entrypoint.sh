#!/bin/sh
# Beim ersten Start die Beispielkonfiguration nach /data legen, dann den Server starten.
set -e
if [ ! -f /data/config.toml ]; then
  cp /opt/crabsdr/config.example.toml /data/config.toml
  echo "crabSDR: /data/config.toml angelegt (Beispiel) – bitte Frequenz/Gain anpassen und Container neu starten."
fi
exec /usr/local/bin/crabsdr-server
