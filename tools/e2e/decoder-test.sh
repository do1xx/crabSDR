#!/bin/sh
# decoder-test.sh — APRS und FT8 Ende zu Ende prüfen: künstliches 2-m-Band (decoder-signals.py) → iqplay → crabSDR
# (volles Image mit Plugins) → /api/decoders/events. Läuft auf einem Rechner mit Docker (Mac Studio).
#   sh tools/e2e/decoder-test.sh [image]    Erwartung je Minute: 6 APRS-Pakete, 8 FT8-Decodes (4 Zyklen × 2)
set -e
IMG=${1:-crabsdr:full-arm64}; W=${W:-$HOME/build/dectest}; PORT=18097
mkdir -p $W/data; D=$(cd "$(dirname "$0")" && pwd)
cp $D/decoder-signals.py $D/../phase0/iqplay.py $W/
cat > $W/data/config.toml <<CFG
port = 8080
frontend_dir = "/opt/crabsdr/web"
data_dir = "/data"
db_path = "/data/crabsdr.db"
plugin_dir = "/opt/crabsdr/plugins"
[[decoders]]
plugin = "aprs"
freq = 144800000
[[decoders]]
plugin = "ft8"
freq = 144174000
[[sdrs]]
id = "2m"
sdr_driver = "rtl_tcp"
sdr_tcp_host = "127.0.0.1"
sdr_tcp_port = 7930
center_freq = 145000000
sample_rate = 2048000
guest = true
CFG
[ -f $W/test2m.cu8 ] || docker run --rm -v $W:/work --entrypoint python3 $IMG /work/decoder-signals.py /work/test2m
docker rm -f dectest >/dev/null 2>&1 || true
rm -rf $W/data/decoders
docker run -d --name dectest -v $W:/work -v $W/data:/data -p $PORT:8080 --entrypoint sh $IMG \
  -c "python3 /work/iqplay.py /work/test2m.cu8 --port 7930 --wallclock > /work/iqplay.log 2>&1 & sleep 1; exec /usr/local/bin/docker-entrypoint.sh" >/dev/null
echo "läuft, warte 150 s …"; sleep 150
curl -s "localhost:$PORT/api/decoders/events?limit=1000" | python3 -c '
import json, sys
ev = json.load(sys.stdin)["events"]
a = [e["data"] for e in ev if e["plugin"] == "aprs"]; f = [e["data"] for e in ev if e["plugin"] == "ft8"]
print("APRS: %d Pakete, %d verschieden, mit Position %d, Pegel %s" % (len(a), len(set(x["src"] for x in a)), sum(1 for x in a if x["lat"] is not None), sorted(set(x["level"] for x in a))[:4]))
print("FT8:  %d Decodes, Nachrichten %s, SNR %s dB, dt %s s" % (len(f), sorted(set(x["msg"] for x in f)), sorted(set(x["snr"] for x in f)), sorted(set(x["dt"] for x in f))))
ok = len(set(x["src"] for x in a)) == 6 and len(set(x["msg"] for x in f)) == 2 and len(f) >= 8
print("ERGEBNIS:", "OK" if ok else "FEHLER"); sys.exit(0 if ok else 1)'
