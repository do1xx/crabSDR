#!/bin/sh
# build.sh — crab_rade bauen (RADE-Decoder für die Betriebsart DV). Holt rade_c (BSD-2-Clause) und Opus mit FARGAN in
# festgelegten Ständen, baut Opus wie rade_c/cmake/BuildOpus.cmake (mit den beiden nnet-Patches) und übersetzt crab_rade.c
# zusammen mit den Bibliotheksquellen von rade_c zu einem einzigen Programm. Braucht: C-Compiler, make, autoconf, automake,
# libtool, patch, curl, unzip, git (Debian: apt install build-essential autoconf automake libtool patch curl unzip git).
#   plugins/rade/build.sh [zielordner]            → <zielordner>/crab_rade (Standard: plugins/rade)
# Umgebung:  CC (Compiler), HOST (Cross-Ziel für Opus-configure, z. B. aarch64-unknown-linux-musl; Compiler dann $HOST-gcc),
#            STATIC=1 (statisch linken, Linux), JOBS (Parallelität), BUILD_DIR (Standard plugins/rade/build),
#            RADE_C_DIR (vorhandenes rade_c statt Klon), OPUS_DIR (fertig gebautes Opus mit .libs/libopus.a statt Neubau)
set -e
cd "$(dirname "$0")"
RADE_COMMIT=c8a3dc156045cae2cd251e1a4be0c304c9ddf2f9      # github.com/freedv/rade_c, 2026-10
OPUS_COMMIT=940d4e5af64351ca8ba8390df3f555484c567fbb      # github.com/xiph/opus, Stand wie rade_c/cmake/BuildOpus.cmake
OUT=${1:-$PWD}
B=${BUILD_DIR:-$PWD/build}; mkdir -p "$B"
JOBS=${JOBS:-4}
if [ -n "$HOST" ]; then CC=${CC:-$HOST-gcc}; else CC=${CC:-cc}; fi

if [ -z "$RADE_C_DIR" ]; then
  RADE_C_DIR=$B/rade_c
  if [ ! -d "$RADE_C_DIR/src" ]; then echo "== rade_c holen ($RADE_COMMIT)"; git clone -q https://github.com/freedv/rade_c "$RADE_C_DIR"; fi
  git -C "$RADE_C_DIR" checkout -q $RADE_COMMIT
else
  H=$(git -C "$RADE_C_DIR" rev-parse HEAD 2>/dev/null || echo unbekannt)
  [ "$H" = "$RADE_COMMIT" ] || echo "Hinweis: $RADE_C_DIR steht auf $H, erwartet $RADE_COMMIT"
fi
SRC=$RADE_C_DIR/src

if [ -z "$OPUS_DIR" ]; then
  OPUS_DIR=$B/opus-${HOST:-native}
  if [ ! -f "$OPUS_DIR/.libs/libopus.a" ]; then
    echo "== Opus mit FARGAN bauen ($OPUS_COMMIT${HOST:+, Ziel $HOST})"
    [ -f "$B/opus-$OPUS_COMMIT.zip" ] || curl -fsSL -o "$B/opus-$OPUS_COMMIT.zip" "https://github.com/xiph/opus/archive/$OPUS_COMMIT.zip"
    rm -rf "$OPUS_DIR" "$B/opus-unzip"; mkdir -p "$B/opus-unzip"
    (cd "$B/opus-unzip" && unzip -q "../opus-$OPUS_COMMIT.zip") && mv "$B"/opus-unzip/opus-* "$OPUS_DIR" && rmdir "$B/opus-unzip"
    (cd "$OPUS_DIR" && patch -s dnn/nnet.h < "$SRC/opus-nnet.h.diff" && patch -s dnn/nnet.c < "$SRC/opus-nnet.c.diff" \
      && ./autogen.sh > autogen.log 2>&1 \
      && CC=$CC ./configure --with-pic --enable-osce --enable-dred --disable-shared --disable-doc --disable-extra-programs ${HOST:+--host=$HOST} > configure.log 2>&1 \
      && make -j"$JOBS" > make.log 2>&1) || { echo "Opus-Build fehlgeschlagen, siehe $OPUS_DIR/*.log"; exit 1; }
  fi
fi
[ -f "$OPUS_DIR/.libs/libopus.a" ] || { echo "$OPUS_DIR/.libs/libopus.a fehlt"; exit 1; }

# Bibliotheksquellen wie add_library(rade …) in rade_c/src/CMakeLists.txt
LIBSRC="rade_api.c rade_enc.c rade_dec.c rade_enc_data.c rade_dec_data.c rade_dsp.c rade_ofdm.c rade_bpf.c rade_acq.c rade_tx.c rade_rx.c
        rade_enc_v2.c rade_dec_v2.c rade_sync.c rade_enc_v2_data.c rade_dec_v2_data.c rade_sync_data.c rade_v2_ofdm.c rade_tx_v2.c rade_rx_v2.c"
OBJ=$B/obj-${HOST:-native}; mkdir -p "$OBJ"
CFLAGS="-O3 -DNDEBUG -std=gnu11 -I$SRC -I$OPUS_DIR/dnn -I$OPUS_DIR/celt -I$OPUS_DIR/include -I$OPUS_DIR"
echo "== crab_rade übersetzen ($CC${STATIC:+, statisch})"
OBJS=""
for f in $LIBSRC; do
  o=$OBJ/${f%.c}.o; OBJS="$OBJS $o"
  [ -f "$o" ] && [ "$o" -nt "$SRC/$f" ] && continue
  $CC $CFLAGS -c "$SRC/$f" -o "$o"
done
$CC $CFLAGS -c crab_rade.c -o "$OBJ/crab_rade.o"
mkdir -p "$OUT"
$CC ${STATIC:+-static} -o "$OUT/crab_rade" "$OBJ/crab_rade.o" $OBJS "$OPUS_DIR/.libs/libopus.a" -lm
ls -la "$OUT/crab_rade"
