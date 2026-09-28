#!/bin/sh
# Frische Teststation für security_test.py und admin.spec.mjs: eigener Temp-Ordner (Konfiguration + Daten), damit
# die Tests nie testdata/ verändern. 70cm-oben = Mitglieder, 10ghz = nur Admin, Decoder aprs (öffentlich) und
# sstv (nicht öffentlich). Startet crabsdr-server auf Port 8090 und gibt das Startpasswort von „admin“ aus.
#   PW=$(tools/e2e/admin-env.sh)        Aufräumen: tools/e2e/admin-env.sh stop
set -e
R=$(cd "$(dirname "$0")/../.." && pwd)
D=${CRABSDR_ADMIN_TEST_DIR:-${TMPDIR:-/tmp}/crabsdr-admin-test}
B=$R/backend-rs/target/release/crabsdr-server
[ -f "$D/pid" ] && kill "$(cat "$D/pid")" 2>/dev/null || true
[ "$1" = stop ] && { rm -rf "$D"; exit 0; }
rm -rf "$D"; mkdir -p "$D/data/decoders/aprs-144800"
echo "PASSCODE geheim" > "$D/data/decoders/aprs-144800/direwolf.conf"
echo '{"ts":1,"packets":[],"stations":[]}' > "$D/data/decoders/aprs-144800/aprs.json"
sed -e "s#^port = 8082#port = 8090#" -e "s#^frontend_dir = \"web\"#frontend_dir = \"$R/web\"#" -e "s#^plugin_dir = \"plugins\"#plugin_dir = \"$R/plugins\"#" \
    -e "s#^site_dir = \"testdata/site-demo\"#site_dir = \"$R/testdata/site-demo\"#" -e "s#^data_dir = \"testdata/data-ui\"#data_dir = \"$D/data\"#" \
    -e 's#^id = "70cm-oben"#id = "70cm-oben"\nguest = false#' -e 's#^id = "10ghz"#id = "10ghz"\nadmin_only = true#' \
    "$R/testdata/config-ui.toml" > "$D/config.toml"
printf '\n[[decoders]]\nplugin = "aprs"\nfreq = 144800000\nenabled = false\n\n[[decoders]]\nplugin = "sstv"\nfreq = 145800000\nenabled = false\npublic = false\n' >> "$D/config.toml"
"$B" "$D/config.toml" > "$D/server.log" 2>&1 &
echo $! > "$D/pid"
for i in 1 2 3 4 5 6 7 8 9 10; do grep -q "Passwort:" "$D/server.log" 2>/dev/null && break; sleep 0.5; done
sed -n 's/.*Passwort: \([A-Za-z0-9]*\).*/\1/p' "$D/server.log" | head -1
