#!/bin/sh
# build-binaries.sh — statische crabsdr-server-Binaries (musl) für x86-64, ARM64 und ARMv7 bauen.
# Läuft auf einem Rechner mit Docker (getestet: Mac Studio, amd64-Images per Rosetta). Ergebnis: dist/bin/<arch>/crabsdr-server
#   [CRABSDR_GIT=<git-stand>] packaging/build-binaries.sh [amd64 arm64 armv7]   (Git-Stand erscheint in --version und ui.json)
# libopus wird im Container selbst gebaut (Quellen aus audiopus_sys) ohne Stack-Schutz, und libc/libm werden ans Ende
# der Link-Zeile gehängt – sonst fehlen beim statischen Linken __stack_chk_fail, sqrt usw.
set -e
cd "$(dirname "$0")/.."
ARCHS=${*:-"amd64 arm64 armv7"}
cat > backend-rs/.musl-build.sh <<'IN'
set -e
command -v cmake >/dev/null || (apt-get update -qq >/dev/null && apt-get install -y -qq cmake >/dev/null)
cargo fetch -q
OSRC=$(ls -d /root/.cargo/registry/src/*/audiopus_sys-0.2.2/opus | head -1)
OP=$PWD/target/opus-pic-$T
if [ ! -f "$OP/lib/libopus.a" ]; then
  cmake -S "$OSRC" -B /tmp/ob -DCMAKE_BUILD_TYPE=Release -DOPUS_STACK_PROTECTOR=OFF -DBUILD_SHARED_LIBS=OFF -DCMAKE_POSITION_INDEPENDENT_CODE=ON \
    -DCMAKE_C_COMPILER=$T-gcc -DCMAKE_SYSTEM_NAME=Linux -DCMAKE_INSTALL_PREFIX=$OP -DCMAKE_INSTALL_LIBDIR=lib >/dev/null
  cmake --build /tmp/ob -j4 >/dev/null && cmake --install /tmp/ob >/dev/null
fi
[ -f target/.opus-linked3-$T ] || { cargo clean -q --release --target $T -p audiopus_sys || true; touch target/.opus-linked3-$T; }
export RUSTFLAGS="-C link-arg=-lc -C link-arg=-lm" OPUS_LIB_DIR=$OP OPUS_STATIC=1 OPUS_NO_PKG=1
cargo build --release --target $T 2>&1 | grep -E "^error|error\[|Finished|undefined reference" | head -5 || true
mkdir -p dist-out/$A && cp target/$T/release/crabsdr-server dist-out/$A/
chown -R "${HOST_UID:-0}:${HOST_GID:-0}" dist-out   # unter Linux (z. B. GitHub) sonst root-eigen, der Aufrufer könnte sie nicht verschieben
IN
for a in $ARCHS; do
  case $a in
    amd64) T=x86_64-unknown-linux-musl; IMG=ghcr.io/rust-cross/rust-musl-cross:x86_64-musl ;;
    arm64) T=aarch64-unknown-linux-musl; IMG=ghcr.io/rust-cross/rust-musl-cross:aarch64-musl ;;
    armv7) T=armv7-unknown-linux-musleabihf; IMG=ghcr.io/rust-cross/rust-musl-cross:armv7-musleabihf ;;
    *) echo "unbekannte Architektur $a"; exit 1 ;;
  esac
  echo "== $a ($T)"
  docker run --rm --platform linux/amd64 -v "$PWD/backend-rs:/home/rust/src" -v "crabsdr-musl-cargo-$a:/root/.cargo/registry" \
    -v "crabsdr-musl-target-$a:/home/rust/src/target" -w /home/rust/src -e T=$T -e A=$a -e CRABSDR_GIT="${CRABSDR_GIT:-}" -e HOST_UID="$(id -u)" -e HOST_GID="$(id -g)" "$IMG" sh .musl-build.sh
  mkdir -p dist/bin/$a && mv backend-rs/dist-out/$a/crabsdr-server dist/bin/$a/ && rmdir backend-rs/dist-out/$a
  ls -la dist/bin/$a/crabsdr-server
done
rmdir backend-rs/dist-out 2>/dev/null || true; rm -f backend-rs/.musl-build.sh
