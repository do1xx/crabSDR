#!/bin/sh
# Oberflächentests ohne Hardware: Testsignal erzeugen, als rtl_tcp abspielen, zwei Testserver starten, alle Prüfungen.
#   tools/e2e/run.sh            (einmalig vorher: cd tools/e2e && npm install && npx playwright install chromium)
# Eigene Mitschnitte (.cu8, 2,048 MS/s) gehen auch: IQ2M=… IQ70=… tools/e2e/run.sh
set -e
cd "$(dirname "$0")/../.."
[ -f testdata/synth.cu8 ] || python3 tools/phase0/synth.py testdata/synth.cu8 --sec 30
B=backend-rs/target/release/crabsdr-server
[ -x $B ] || (cd backend-rs && cargo build --release -p crabsdr-server)
P=""
trap 'kill $P 2>/dev/null' EXIT
python3 tools/phase0/iqplay.py "${IQ2M:-testdata/synth.cu8}" --port 7901 >/dev/null 2>&1 & P="$P $!"
python3 tools/phase0/iqplay.py "${IQ70:-testdata/synth.cu8}" --port 7903 >/dev/null 2>&1 & P="$P $!"
python3 tools/phase0/iqplay.py testdata/synth.cu8 --port 7905 >/dev/null 2>&1 & P="$P $!"
CRABSDR_CONFIG=testdata/config-ui.toml $B >testdata/ui.log 2>&1 & P="$P $!"
CRABSDR_CONFIG=testdata/config-ui2.toml $B >testdata/ui2.log 2>&1 & P="$P $!"
sleep 3
F=0
for t in "features.spec.mjs http://127.0.0.1:8083" ui.spec.mjs bandpick.spec.mjs squelch.spec.mjs share.spec.mjs decmenu.spec.mjs; do
  if node tools/e2e/$t >/tmp/crabsdr-e2e.log 2>&1; then echo "ok    $t"; else echo "FEHL  $t"; grep -E '^FEHL' /tmp/crabsdr-e2e.log | head -5; F=1; fi
done
# Anmeldung, Rechte, Admin-Seite: eigene Teststation (Port 8090, Temp-Ordner)
PW=$(sh tools/e2e/admin-env.sh)
if python3 tools/e2e/security_test.py http://127.0.0.1:8090 "$PW" >/tmp/crabsdr-e2e.log 2>&1; then echo "ok    security_test.py"; else echo "FEHL  security_test.py"; grep -E '^FEHL' /tmp/crabsdr-e2e.log | head -5; F=1; fi
PW=$(sh tools/e2e/admin-env.sh)
if node tools/e2e/admin.spec.mjs http://127.0.0.1:8090 "$PW" >/tmp/crabsdr-e2e.log 2>&1; then echo "ok    admin.spec.mjs"; else echo "FEHL  admin.spec.mjs"; grep -E '^FEHL|FEHLER' /tmp/crabsdr-e2e.log | head -5; F=1; fi
sh tools/e2e/admin-env.sh stop
exit $F
