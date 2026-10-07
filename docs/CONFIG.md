# crabSDR – Konfiguration

Eine Datei im TOML-Format. Für eine Station reichen **Station + ein Band**; alles andere hat Voreinstellungen,
die für die meisten passen. Wer mehr will, findet hier jede Einstellung.

```toml
[station]
name = "Mein WebSDR"
locator = "JO53RB"

[[bands]]
id = "2m"
driver = "rtl_sdr"
center_freq = 145000000
gain = 30
```

Eine kommentierte Vorlage liegt in `packaging/config.example.toml` (im Release: `config.example.toml`).

## Wo die Datei liegt, prüfen, neu starten

crabSDR nimmt die erste vorhandene Datei aus:

1. dem Pfad als Argument: `crabsdr-server /pfad/config.toml`
2. der Umgebungsvariable `CRABSDR_CONFIG`
3. `/etc/crabsdr/config.toml` (Installation mit `install.sh`)
4. `/data/config.toml` (Docker)
5. `./config.toml`

Vor jedem Neustart prüfen:

```bash
crabsdr-server --check /etc/crabsdr/config.toml
```

Die Prüfung meldet Tippfehler in Schlüsselnamen, Decoder außerhalb der Bänder, fehlende Plugins und Programme,
nicht erreichbare `rtl_tcp`-Quellen und fehlende Ordner. Rückgabe 0 heißt: startklar. Ist die Datei nicht lesbar
oder fehlerhaft, startet crabSDR **nicht** (mit Zeile und Spalte des Fehlers im Journal), statt still mit
Voreinstellungen zu laufen. Die Installationsskripte prüfen vor dem Neustart und lassen bei Fehlern die laufende
Station unangetastet.

`crabsdr-server --version` zeigt die Version.

## Oben in der Datei

| Schlüssel | Voreinstellung | Bedeutung |
|---|---|---|
| `port` | `8080` | HTTP-Port der Seite |
| `site_dir` | – | Stationsordner, siehe unten |
| `frontend_dir` | automatisch | Oberfläche; neben dem Programm (`…/bin/crabsdr-server` → `…/web`), sonst `/opt/crabsdr/web`, sonst `./web` |
| `plugin_dir` | automatisch | Decoder-Plugins; wie oben mit `plugins` |
| `data_dir` | automatisch | Daten (Chat, Logbuch, Decoder-Ergebnisse, Benutzer): `/data` in Docker, sonst `/var/lib/crabsdr`, sonst `./data` |
| `db_path` | `data_dir/crabsdr.db` | Benutzerdatenbank (Admin, Gäste) |
| `builtin_chat` | `true` | Chat und Logbuch der Hörer (SQLite `data_dir/pinnwand.db`). `false` nur, wenn ein anderer Dienst sie übernimmt |
| `chat_keep_lines` | `50` | Chat behält nur die letzten so vielen Zeilen (FIFO), `0` = alle behalten. Das Logbuch bleibt |
| `max_listeners` | `50` | Verbindungen je Band insgesamt (Browser und Streams), `0` = keine Grenze |
| `max_per_ip` | `10` | Verbindungen je Band und Absenderadresse (mehrere Tabs/Bänder einer Person zählen einzeln) |
| `max_channels` | `16` | verschiedene Kanäle je Band: jede andere Frequenz, Betriebsart oder Bandbreite kostet einen DSP-Kanal, Hörer auf demselben Kanal teilen ihn. Darüber: „Station voll“ |
| `max_streams` | `6` | gleichzeitige Streams (`/stream/…`) je Band |
| `iq_stream` | `admin` | I/Q-Stream (`iq.wav`/`iq.ogg`, komplexes Basisband für externe Decoder): `off`, `admin` (nur Sysop), `users` (angemeldete Hörer), `all` |
| `max_iq` | `1` | gleichzeitige I/Q-Streams je Band (ab 0,5 Mbit/s je Hörer) |
| `stream_key` | leer | fester Schlüssel für Streams: `?token=<stream_key>` zählt wie der Sysop, nur für `/stream/…`, läuft nicht ab. Lang und zufällig, bei Verlust ändern |
| `chat_keep_hours` | `3` | Zusätzlich Chat-Zeilen nach so vielen Stunden löschen, `0` = aus |
| `chat_verbund` | `false` | Verbund-Chat: Chatzeilen (Name, Text, Uhrzeit, Stationskürzel) über crabsdr.de mit allen teilnehmenden Stationen teilen. Die Station reicht weiter, Hörer-IPs verlassen sie nicht; dort 24 h. Braucht `[directory] enabled` |
| `opus_bitrate` | `32000` | bit/s je belegter Frequenz (Opus, 24 kHz mono); 32 k reicht für Sprache, 48 k für Rundfunk |
| `opus_complexity` | `3` | Opus-Rechenaufwand 0–10; 3 genügt für Sprache und spart CPU |

Umgebungsvariablen überschreiben: `PORT`, `PLUGIN_DIR`, `FRONTEND_DIR`, `DATA_DIR`.

## `[station]` – wer sendet die Seite

| Schlüssel | Bedeutung |
|---|---|
| `name` | Titel oben links, Seitentitel, Link-Vorschau (Voreinstellung `crabSDR`) |
| `subtitle` | Zeile unter dem Titel, z. B. Ort, Antenne, Höhe |
| `locator` | Maidenhead-Locator (4, 6 oder 8 Zeichen) |
| `lat`, `lon` | Standort in Dezimalgrad; ohne Angabe die Mitte des Locators. Dient für Entfernungen im Logbuch, auf der Digital-Seite, für die Reichweite und als Standort der APRS-iGate-Bake |
| `url` | öffentliche Adresse, z. B. `https://sdr.example.org` (optional; nötig für das Verzeichnis) |
| `operator`, `address`, `contact` | Betreiber (Name/Rufzeichen), Anschrift (Zeilen mit ` · ` trennen), E-Mail. Mit `operator` zeigt die Info-Seite ein Impressum, der Fuß verlinkt es. Der Datenschutz-Hinweis auf der Info-Seite ist immer da (`[ui] datenschutz` überschreibt den Link) |

## `[[bands]]` – ein Eintrag je Empfänger

Jede Tabelle `[[bands]]` ist ein Band mit eigenem Wasserfall. Hörer auf derselben Frequenz kosten praktisch
nichts; jede weitere belegte Frequenz etwa 0,4 % CPU (x86), jedes Band mit 2,048 MS/s etwa 4 %.

| Schlüssel | Voreinstellung | Bedeutung |
|---|---|---|
| `id` | – (Pflicht) | kurzer Name ohne Leerzeichen, erscheint in Adressen und gespeicherten Einstellungen der Hörer |
| `label` | `id` | Anzeige in der Bandleiste |
| `note` | – | Kurzbeschreibung (Tooltip, Info-Seite) |
| `driver` | `rtl_tcp` | Quelle: `rtl_sdr` (Stick am Rechner), `rtl_tcp` (Stick über das Netz), `hackrf`, `rx_sdr` (SoapySDR) |
| `device` | `0` | Index oder Seriennummer des Sticks (`rtl_sdr`, `hackrf`); bei `rx_sdr` die SoapySDR-Angabe, z. B. `driver=soapyMiri` oder `driver=airspy,serial=…` (siehe unten) |
| `host`, `port` | `127.0.0.1`, `1234` | Adresse des `rtl_tcp`-Servers |
| `center_freq` | – (Pflicht) | Mitte des Wasserfalls in Hz |
| `sample_rate` | `2048000` | Abtastrate; sichtbare Breite ≈ Abtastrate |
| `gain` | `40` | Verstärkung in dB. Übersteuert der Stick (Geisterträger, springender Rauschteppich), kleiner stellen |
| `ppm` | `0` | Frequenzkorrektur des Quarzes |
| `mode` | – | Betriebsart beim ersten Öffnen des Bandes: `fm`, `am`, `usb`, `lsb`, `cw` |
| `smeter_cal` | – | S-Meter-Kalibrierung in dB: dBm = Kanalpegel (dBFS) − `gain` + `smeter_cal`. Messen: leerer Kanal oder Messsender, K = Soll-dBm − (dBFS − gain) |
| `bias_tee` | `false` | Speisespannung für einen Vorverstärker am Antenneneingang |
| `enabled` | `true` | `false` = Band aus, ohne den Eintrag zu löschen |
| `guest` | `true` | Zugang: `true` = öffentlich (ohne Anmeldung), `false` = nur für angemeldete Benutzer, denen das Band zugeteilt ist |
| `admin_only` | `false` | nur für Admins (hat Vorrang vor `guest`) |
| `fft_size` | automatisch | Punkte der FFT; automatisch so, dass ein Bin etwa 500 Hz breit ist (2,048 MS/s → 4096, 8 MS/s → 16384). Deutlich kleiner bei hoher Abtastrate lässt FM pfeifen |
| `fft_fps` | `50` | Wasserfall-Zeilen pro Sekunde |
| `gain_elements` | – | Verstärkerstufen einzeln, z. B. `{ LNA = 24, Baseband = 30 }` (SoapySDR-Geräte) |
| `format` | `cs16` | nur `rx_sdr`: Sample-Format – `cs16` (16 Bit, volle Dynamik), `cf32` (Module, die nur Gleitkomma liefern), `cu8` (8 Bit, halbe Datenmenge). Bei 16 Bit und Gleitkomma zieht crabSDR den Gleichanteil ab (Mittenspitze bei Null-ZF-Empfängern) |
| `freq_correction_ppm` | `0` | Frequenzkorrektur in Software, ppm mit Nachkommastellen, für jeden Treiber: verschiebt Anzeige und Abstimmung. Positiv, wenn bekannte Signale zu tief erscheinen (z. B. Relaisausgabe 145,700 liest 145,69985 → `1.03`). Für Empfänger ohne TCXO wie den RSP1 |
| `settings` | – | nur `rx_sdr`: Geräte-Einstellungen für SoapySDR (`rx_sdr -t`), z. B. `"biastee=true"`; MSi2500/RSP1 mit `"transfer=BULK"` (USB-Bulk statt isochron, stabiler am Pi 4) braucht SoapyMiri mit [packaging/soapymiri-transfer.patch](../packaging/soapymiri-transfer.patch); `"flavour=SDRplay"` beim echten RSP1 (Preselektor-Filter nach dessen Plan, ohne die Angabe gilt der Plan des nackten MSi2500) |

Alte Schlüsselnamen gelten weiter, `--check` weist darauf hin: `[[sdrs]]` → `[[bands]]`, `sdr_driver` → `driver`,
`sdr_device` → `device`, `sdr_tcp_host` → `host`, `sdr_tcp_port` → `port`, `default_mode` → `mode`, die Tabelle
`[smeter_cal]` → `smeter_cal` im Band.

### Empfänger

| Gerät | `driver` | `device` | Treiber (frei) |
|---|---|---|---|
| RTL-SDR-Stick | `rtl_sdr` | Index oder Seriennummer | Paket `rtl-sdr` |
| RTL-SDR im Netz | `rtl_tcp` | – (`host`, `port`) | `rtl_tcp` auf dem anderen Rechner |
| HackRF | `hackrf` | Index | Paket `hackrf` |
| Airspy, Airspy HF+ | `rx_sdr` | `driver=airspy` bzw. `driver=airspyhf` | SoapySDR-Module aus Debian |
| MSi2500 / SDRplay RSP1 und Nachbauten | `rx_sdr` | `driver=soapyMiri` | [libmirisdr-5](https://github.com/ericek111/libmirisdr-5) und [SoapyMiri](https://github.com/ericek111/SoapyMiri) |
| LimeSDR, PlutoSDR, bladeRF, USRP | `rx_sdr` | `driver=lime`, `driver=plutosdr`, … | SoapySDR-Module |

`rx_sdr` stammt aus [rx_tools](https://github.com/rxseger/rx_tools) (im Docker-Image enthalten). Welche Geräte
SoapySDR findet und was ins Band gehört, zeigt die Admin-Seite unter „Sticks“ (oder `SoapySDRUtil --find`).
Das alte Debian-Modul `driver=miri` für den MSi2500 verliert Samples und stürzt ab – dafür libmirisdr-5 und SoapyMiri
aus dem Quellcode bauen (`cmake`, `make install`; beide nach `/usr/local`, die alte `libmirisdr0` stört nicht).

## `[[decoders]]` – Decoder hören ständig mit

Ein Eintrag = ein Plugin auf einer Frequenz. Das Plugin (`plugin_dir/<name>/decoder.json`) weiß selbst, welche
Betriebsart, Abtastrate und Bandbreite es braucht; die Konfiguration sagt nur, wo es hören soll. Ergebnisse
erscheinen auf der Seite **Digital** und unter `/api/decoders`.

| Schlüssel | Voreinstellung | Bedeutung |
|---|---|---|
| `plugin` | – (Pflicht) | `aprs`, `ft8`, `sstv` (mitgeliefert) |
| `freq` | – (Pflicht) | Frequenz in Hz; das Band ergibt sich daraus |
| `band` | automatisch | Band-`id`, falls sich Bänder überlappen |
| `id` | `<plugin>-<kHz>` | Kennung, zugleich Datenordner `data_dir/decoders/<id>` |
| `label` | Name des Plugins | Anzeige |
| `enabled` | `true` | Decoder aus, Daten bleiben sichtbar |
| `public` | `true` | `false` = Ergebnisse nur für Admins und Benutzer, denen der Decoder zugeteilt ist |
| `options` | – | Einstellungen des Plugins, z. B. `options = { depth = "2" }` |

Mehrere Einträge desselben Plugins sind erlaubt (SSTV auf 145,800 und 144,900).

**APRS** (braucht `direwolf`): `mycall` (Rufzeichen, sonst nur Empfang als N0CALL), `igate = "on"` gibt
Empfangenes an APRS-IS (aprs.fi) weiter, nie per Funk; der Passcode wird aus dem Rufzeichen berechnet
(`igate_call`, `igate_passfile`, `igate_server` = `euro.aprs2.net`). Bake: `symbol` (`R&`), `comment`, `every`
(Minuten, 30), Standort aus `[station]` oder `lat`/`lon`.

**FT8** (braucht `jt9` aus WSJT-X): `depth` (Suchtiefe 1–3, Standard 3; 2 spart auf einem Raspberry Pi viel
Rechenzeit), `days` (Stationen der letzten n Tage, 90), `keep` (Decodes in der Liste, 300).

**SSTV** (braucht `python3-numpy`, `python3-pil`): `keep` (Bilder im Index, 500). Erkennt Martin, Scottie,
Robot 36/72, PD 50–290 (ISS: PD 120).

Plugins bekommen Umgebung: `CRAB_FREQ`, `CRAB_RATE`, `CRAB_MODE`, `CRAB_BAND`, `CRAB_ID`, `CRAB_LABEL`, `CRAB_DATA`,
`CRAB_STATION_NAME`, `CRAB_STATION_LOCATOR`, `CRAB_STATION_LAT`, `CRAB_STATION_LON` und `CRAB_OPT_<OPTION>`.

## `[ui]` – was die Hörer sehen

Alles ist an, solange es etwas zu zeigen gibt. Nur eintragen, was abweichen soll.

| Schlüssel | Voreinstellung | Bedeutung |
|---|---|---|
| `login` | an, wenn es Mitglieder- oder Admin-Bänder gibt | Knopf „Anmelden“ auf der Hören-Seite |
| `chat` | wie `builtin_chat` | Chat auf der Hören-Seite |
| `logbook` | wie `builtin_chat` | Reiter Logbuch |
| `digital` | an, wenn öffentliche Decoder laufen | Reiter Digital |
| `info` | an | Reiter Info (baut sich aus Station, Bändern und Decodern; eigene Seite im Stationsordner) |
| `decoders` | an, wenn Decoder da sind | Decoder-Menü in der Bedienleiste |
| `recording` | an | Aufnahme-Knopf |
| `status` | an | Statuszeile |
| `banner` | – | Hinweis oben auf allen Seiten, z. B. `"Wartung heute ab 20 Uhr"` |
| `impressum`, `datenschutz` | – | Links unten |
| `admin` | `admin/` | Admin-Link unten; nur sichtbar, wenn die Seite über eine private Adresse (LAN, Tailscale) geöffnet wird |

## Stationsordner (`site_dir`)

Optionale `segments.json` ersetzt den eingebauten IARU-Bandplan für die Auswahl „Bereich“ (Wasserfall auf einen
Ausschnitt zoomen): `[{"lo": 144150, "hi": 144400, "label": "SSB", "mode": "usb", "call": 144300}]` in kHz,
optional `"band": "<id>"`.

Ein Ordner mit Dateien, die Vorrang vor der Oberfläche haben. Alles ist optional:

| Datei | Inhalt |
|---|---|
| `logo.svg` | eigenes Logo (die crabSDR-Krabbe erscheint dann klein daneben) |
| `presets.json` | Schnellwahl: `[{band, label, freq (kHz), mode}]` |
| `markers.json` | Marker im Wasserfall |
| `relais.json` | Frequenzliste (Knopf „Liste“): `[{freq (kHz), call, info, shift, tone, mode}]` |
| `positionen.json` | feste Standorte für APRS-Stationen ohne Positionsbake: `{"CALL": {lat, lon, info, ungefaehr}}` |
| `info/index.html` | eigene Info-Seite statt der automatischen |
| `ui.json` | Feineinstellungen der Oberfläche (Farben, Park-Marker …) |

Jede andere Datei der Oberfläche lässt sich genauso ersetzen; wer das tut, muss sie bei Updates selbst pflegen.

## Admin-Seite und Benutzer

Unter `/admin/` verwaltet der Admin die Station im Browser: Überblick (Bänder, Decoder mit ihren Meldungen, Hörer,
Last, Plattenbelegung – rot ab 85 %), Station und Oberfläche, Bänder, Decoder, Benutzer, Chat und Logbuch, die Konfigurationsdatei als Text und die
angeschlossenen Sticks. Formulare und Textansicht schreiben dieselbe `config.toml`: Kommentare bleiben erhalten, vorher
gilt dieselbe Prüfung wie bei `--check`, die vorige Fassung landet in `data_dir/config-backups/`. Die meisten
Änderungen wirken nach „Neu starten“ (über systemd bzw. Docker); die Verstärkung lässt sich sofort ausprobieren.

- **Erster Start:** Das Konto `admin` bekommt ein Zufallspasswort, es steht einmal im Protokoll
  (`journalctl -u crabsdr | grep Passwort`, Docker: `docker compose logs | grep Passwort`) und muss beim ersten
  Anmelden geändert werden. Vergessen: `crabsdr-server --reset-admin` am Rechner.
- **Benutzer:** Rolle Admin oder Nutzer. Nutzer hören alle öffentlichen Bänder und dazu die ihnen zugeteilten
  Mitglieder-Bänder; nicht öffentliche Decoder sehen sie nur, wenn zugeteilt. Sie melden sich auf der Hören-Seite an.
- **Nicht über die Admin-Seite änderbar** sind Port, Pfade (`frontend_dir`, `plugin_dir`, `data_dir`, `db_path`,
  `site_dir`), `jwt_secret_env`, `[site]` und `mqtt.password_file` – nur in der Datei am Rechner.
- **Nur lesen:** Soll die Admin-Seite die Datei nicht ändern, Rechte auf 640 setzen (`chmod 640 /etc/crabsdr/config.toml`);
  mit `install.sh` eingerichtet ist sie für die Gruppe `crabsdr` beschreibbar (660, Ordner 2770).

Regeln für Passwörter, Sitzungen und die Anmeldebremse: [SECURITY.md](SECURITY.md).

## `[mqtt]` – Decoder-Treffer weitergeben

| Schlüssel | Voreinstellung | Bedeutung |
|---|---|---|
| `host`, `port` | –, `1883` | Broker |
| `username` | – | Benutzer |
| `password_file` | – | Datei mit dem Passwort (nur auf dem Gerät, nie in der Konfiguration) |
| `topic` | – | Themenpräfix, z. B. `crabsdr/meinestation`; Treffer unter `<topic>/<decoder-id>/<art>`, `<topic>/status` = online/offline |
| `enabled` | `true` | vorübergehend aus |

## `[directory]` – im Verzeichnis auf crabsdr.de erscheinen

Wie das Receiverbook bei OpenWebRX: Auf crabsdr.de steht eine Liste aller crabSDR-Stationen, die das wollen. Aus, bis der
Sysop es einschaltet (Admin-Seite → Station → „Im Verzeichnis listen“, oder hier).

| Schlüssel | Voreinstellung | Bedeutung |
|---|---|---|
| `enabled` | `false` | Station im Verzeichnis listen; braucht `[station] url` |
| `server` | `https://crabsdr.de` | Verzeichnis (nur in der Datei änderbar) |

Die Station meldet alle 5 Minuten Name, Untertitel, Locator/Standort, die **öffentlichen** Bänder und Decoder, die
Hörerzahl und die Version. Mitglieder- und Admin-Bänder, Benutzer, Chat und Logbuch werden nie gemeldet. Dieselben
Angaben stehen unter `/api/directory` (sonst 404); das Verzeichnis ruft diese Adresse unter `url` selbst ab und
vergleicht die Kennung (Zufallszahl in `data_dir/directory.id`), damit niemand fremde Adressen eintragen kann. Ist
`enabled` wieder aus, verschwindet die Station nach spätestens einer halben Stunde.

## Selten gebraucht

| Schlüssel | Bedeutung |
|---|---|
| `jwt_secret_env` | Name der Umgebungsvariable mit dem JWT-Schlüssel (Standard `CRABSDR_JWT_SECRET`, sonst in `data_dir`) |

Änderungen über die Admin-Schnittstelle schreiben die Konfigurationsdatei neu; Kommentare gehen dabei verloren.
