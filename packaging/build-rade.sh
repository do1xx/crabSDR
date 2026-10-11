#!/bin/sh
# build-rade.sh — crab_rade (RADE-Decoder für die Betriebsart DV, plugins/rade) statisch für Linux bauen: amd64 und arm64,
# in denselben musl-Cross-Containern wie build-binaries.sh (Docker nötig). Ergebnis: dist/bin/<arch>/crab_rade
#   packaging/build-rade.sh [amd64 arm64]
# Das Programm ist vollständig statisch (musl) und läuft auf jedem Linux der Architektur; Opus (mit FARGAN) und rade_c
# kommen in festgelegten Ständen aus plugins/rade/build.sh. ARMv7 bewusst nicht: der neuronale Decoder braucht auf einem
# Pi 4 etwa 20 % eines Kerns, auf 32-Bit-Boards reicht es nicht für Echtzeit.
set -e
cd "$(dirname "$0")/.."
ARCHS=${*:-"amd64 arm64"}
for a in $ARCHS; do
  case $a in
    amd64) T=x86_64-unknown-linux-musl; IMG=ghcr.io/rust-cross/rust-musl-cross:x86_64-musl ;;
    arm64) T=aarch64-unknown-linux-musl; IMG=ghcr.io/rust-cross/rust-musl-cross:aarch64-musl ;;
    *) echo "unbekannte Architektur $a (amd64, arm64)"; exit 1 ;;
  esac
  echo "== crab_rade $a ($T)"
  docker run --rm --platform linux/amd64 -v "$PWD/plugins/rade:/work" -v "crabsdr-rade-build-$a:/work/build" -w /work \
    -e HOST=$T -e STATIC=1 -e JOBS=4 -e HOST_UID="$(id -u)" -e HOST_GID="$(id -g)" "$IMG" sh -c '
      set -e
      for p in autoconf automake libtool patch curl unzip git make; do command -v $p >/dev/null 2>&1 || MISSING="$MISSING $p"; done
      if [ -n "$MISSING" ]; then apt-get update -qq >/dev/null && apt-get install -y -qq $MISSING >/dev/null; fi
      git config --global --add safe.directory "*"
      sh build.sh /work/build/out-$HOST
      chown -R "$HOST_UID:$HOST_GID" /work/build/out-$HOST'
  # Build-Ordner liegt im Docker-Volume (Opus-Build wird so wiederverwendet): Ergebnis aus dem Volume holen
  mkdir -p dist/bin/$a
  docker run --rm -v "crabsdr-rade-build-$a:/b" "$IMG" cat /b/out-$T/crab_rade > dist/bin/$a/crab_rade
  chmod 755 dist/bin/$a/crab_rade; ls -la dist/bin/$a/crab_rade
done
