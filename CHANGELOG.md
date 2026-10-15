# Änderungen

Versionen nach [Semantic Versioning](https://semver.org/lang/de/). `crabsdr-server --version` zeigt Version und Git-Stand.

## Unveröffentlicht

### Decoder
- Pager-Seite: Rufliste nutzt die Fensterhöhe; Klick auf einen RIC rechts zeigt nur dessen Rufe, bis zu 60 RICs.
- APRS-iGate kann ein zusätzliches Objekt melden (`object_name`, `object_comment`, …), z. B. den Clubabend als eigenen Punkt auf der APRS-Karte.
- **Pager/POCSAG live:** Plugins können eine Vorgabefrequenz nennen (`freq` in `decoder.json`); ein `[[decoders]]`-Eintrag ohne `freq` übernimmt sie, ohne Vorgabe meldet die Prüfung „freq fehlt“. `pocsag` bringt 439,9875 MHz (DAPNET) mit – `plugin = "pocsag"` genügt, wenn das 70-cm-Band die Frequenz abdeckt. Die Decoder-Karte im Menü zeigt Rufe lesbar (RIC, Funktion, Baudrate, Text bzw. „numerisch“/„Tonruf“; vorher nur der Text, bei Tonrufen rohes JSON). Doku: CONFIG.md (Optionen `baud`, `keep`, `charset`), READMEs, config.example.toml.

## 1.5.0 – 2026-10-15

RADE-Rufzeichen, SSTV-Korrektur, neuestes Bild als feste Adresse, Rechtstexte, Rauschsperre ohne Fahne, FM-Klang,
Ogg-Stream im Browser und im Auto, Ton auf iPhone, Handy-Ansicht mit Querformat.

### Handy-Ansicht (iPhone und Android)
- **Querformat ist jetzt eine Handy-Ansicht.** Ein Handy quer (850–930 px breit, nur 350–400 px hoch) landete bisher in der
  Desktop-Ansicht: dreizeilige Bedienleiste, Wasserfall als schmaler Streifen. Jetzt gilt „Handy“ auch bei Touch und
  weniger als 500 px Höhe (gleiche Abfrage in `ui.css`, `look.css` und `ui.js`): Bandleiste oben, Wasserfall über die
  volle Höhe, die Bedienleiste als schmale Spalte rechts (Frequenz, Betriebsart, SQ, Stumm, Menü, Ton, −/+, Chat),
  Notch und Rand über `safe-area`. Reiter (Logbuch, Info) und Sprache stehen im Querformat oben im Menü. Drehen baut
  die Ansicht ohne Neuladen um – Ton und Verbindung laufen weiter.
- **Wasserfall mit dem Finger:** Tippen stimmt ab (beim Loslassen, mit Raster), Ziehen mit einem Finger verschiebt den
  Ausschnitt wie auf einer Karte (bisher ging das nur über Zoom raus/rein), zwei Finger zoomen – jetzt eine Stufe je
  Verdopplung des Fingerabstands (vorher vier Stufen je Geste).
- **Leiste unten:** alle Ziele 44 px hoch (vorher 30), zweite Zeile mit Schritt-Knöpfen −/+ (ein Kanal im Raster), Pegel
  und Chat-Knopf mit Zähler – der Chat war auf dem Handy nur über das Menü erreichbar. Der Wasserfall lag 10 px unter der
  Leiste (die feste Leiste wurde nicht mitgerechnet), jetzt endet er darüber.
- **Menü (Einstellungen):** Knöpfe, Auswahlfelder und Umschalter mindestens 40 px (look.css hatte 30 px), Schnellwahl und
  „Liste“ nebeneinander, Bereich darunter, Squelch und Stumm nebeneinander mit dem Regler darunter, Frequenzfeld 16 px
  (iOS zoomt sonst hinein), Griff im Kopf. Neu: Karte „Decoder“ mit Zustand, letzten Treffern, „Hören“ und dekodiertem
  Ton – wie das Decoder-Menü der Desktop-Leiste (Stationen mit Decodern).
- **Tastatur:** öffnet sich die Tastatur (Frequenz, Name, Chat), heftet sich das Menü bzw. der Chat an den sichtbaren
  Ausschnitt (`visualViewport`) – auf iOS lag das Feld bisher hinter der Tastatur.
- Reiter und Bandleiste 40 px hoch, aktives Band rollt in der Bandleiste ins Bild, Chat sitzt über der Leiste,
  Hinweise (Toast) darüber, kein Überscrollen/„Pull to refresh“ beim Ziehen im Wasserfall.

### iPhone und iPad
- **Ton auf iOS zuverlässig:** der Stummschalter machte Web-Audio lautlos (eine stille Tonschleife hält die Sitzung jetzt in
  der Wiedergabe-Kategorie), nach Anruf, Siri, Sperre oder App-Wechsel blieb der Ton aus (Zustand „interrupted“ wird jetzt
  bei jeder Berührung und beim Zurückkommen fortgesetzt), und nach einem Anruf lief der Ton mit falscher Abtastrate (Context
  wird neu angelegt).

### Oberfläche
- **Relais-Marker mit Hover-Tafel.** `relais.json` im Stationsordner trägt jetzt `locator` und `qth`; daraus entstehen Marker im
  Wasserfall (Rufzeichen an der Ausgabe; die Eingabe als gedimmte Marke „↑ Rufzeichen“, wo das Band sie zeigt) und beim Überfahren eine
  Tafel mit Ausgabe/Eingabe/Ablage, Ton, QTH, Locator, Entfernung und Richtung von der Station (aus `[station]` locator
  bzw. lat/lon). Bei Gedränge behalten die nahen Relais ihre Beschriftung, die fernen nur die (jetzt breitere, anklickbare)
  Linie – die Tafel geht an beiden. Die Liste hinter „Liste“ zeigt zusätzlich die Richtung. Feld `system` (DMR, C4FM,
  D-Star, TETRA, ATV, DAPNET) kennzeichnet digitale Relais: Linie und ein Strich unter dem Rufzeichen zeigen die Systeme
  als Farbstreifen (FM grün, DMR orange, C4FM blau, D-Star violett, TETRA rot), gleiche Ausgabe mit mehreren Systemen
  (DB0HEW: FM, DMR, D-Star, C4FM) ist eine Marke mit „Regenbogen“, die Tafel listet je System Ton und Info; kleine
  Legende in der Statuszeile. `ui.json` `"crab_brand": false` lässt die crabSDR-Marke neben einem eigenen Logo weg.
- **Admin-Seite: Reiter „Relais & Marker“.** `relais.json` als Tabelle (Zeilen hinzufügen/löschen, Speichern) oder als Text,
  `markers.json` als Text; Endpunkte `GET/PUT /api/admin/site/{relais,markers,presets}.json` mit Prüfsumme gegen
  Überschreiben, Sicherungen unter `data_dir/site-backups/` (die letzten 20). Die Oberfläche liest die Dateien bei jedem
  Laden neu, kein Neustart nötig.
- **RADE-Rufzeichen sichtbar.** Das Rufzeichen, das `crab_rade` am Ende jeder Durchgabe aus dem EOO-Rahmen meldet
  (`{"kind":"text",…}`), erscheint jetzt als Marke neben dem Knopf „DV“ (Tooltip: Mode, SNR, „vor n min“; grau nach
  10 min, weg nach 1 h) – für die gehörte DV-Gruppe, sonst für eine DV-Gruppe auf der eingestellten Frequenz (±3 kHz);
  Anfangszustand beim Laden aus `api/decoders/events`, danach live über den WebSocket. Der Server sendet `text`-Treffer
  einer DV-Gruppe wie `sync` unter der Gruppen-ID (`dv-<Hz>`, `data.plugin`). Das Decoder-Menü zeigt Sync, EOO und
  Rufzeichen der DV-Plugins lesbar statt als JSON.
- **Digital-Seite: Reiter „FreeDV“.** Neu (bisher nur vorbereitet): Liste der gehörten Rufzeichen (Zeit, Rufzeichen,
  Frequenz, Mode, SNR) aus dem Ereignisspeicher der Plugins `rade` und `freedv` – auch von schon beendeten DV-Decodern –,
  dazu die Decoder (feste und flüchtige der Betriebsart DV) und der Codec-2-Zustand aus `freedv.json`. Der Reiter
  erscheint, sobald die Station DV kann oder ein `rade`/`freedv`-Decoder läuft; Deep-Link `digi/?tab=freedv`.
  `/digi/freedv.json` liefert dafür `{status, decoders, calls}` statt nur der Plugin-Datei (nie 404).

### Streams
- **Live-Titel im Ogg-Stream (Ogg-Kettung):** Abspielprogramme sehen statt eines festen Titels den Zustand des Kanals –
  „145700 kHz FM · +18 dB“, „… · Rauschsperre zu“, mit Rufzeichen aus den Decodern (RADE/FreeDV-Durchgabe ±3 kHz, APRS-Absender
  ±10 kHz): „145700 kHz FM · DO1XX · +24 dB“. Der Server kettet dafür Ogg-Ströme wie Icecast beim Titelwechsel (EOS-Seite,
  neuer Strom mit neuer Seriennummer, Granule 0, OpusHead/OpusTags); Rauschabstand in 6-dB-Stufen mit Hysterese, Wechsel
  höchstens alle 5 s. An der Naht geht kein Abtastwert verloren: der alte Kodierer gibt seine Verzögerung mit einem letzten
  Rahmen aus, der neue wird angewärmt und kodiert seinen ersten Rahmen intra (dafür libopus direkt über `audiopus_sys`,
  `OPUS_SET_PREDICTION_DISABLED`) – gemessen am 50-Hz-Ton bleibt der Fehler an der Naht unter −33 dB (`kette_sinus`).
  VLC reicht den Titel an Bluetooth/AVRCP und die Sperrbildschirm-Anzeige weiter (VLC 3 setzt dabei einen Rahmen aus, weil
  es die Spur neu anlegt; VLC 4 nicht), mpv und ffmpeg spielen lückenlos. `?live=0` schaltet die Kettung ab; Browser
  bekommen sie nie. Neu auch `ALBUM=<Station>` in den Tags. docs/STREAM.md „Live-Titel“.
- **Wasserfall als Titelbild:** `?cover=wf` legt alle 10 s ein JPEG 320 × 180 (letzte 18 s Wasserfall um die Frequenz, Kanal rot
  markiert, 3–25 kB) als `METADATA_BLOCK_PICTURE` ins neue Kettenglied – VLC zeigt es als Cover.
- **Ogg-Kopf über mehrere Seiten:** Pakete mit mehr als 255 Segmenten (große OpusTags, z. B. mit Bild) verteilt der Muxer jetzt
  auf Fortsetzungsseiten (Flag 0x01, Granule −1); vorher überlief das Zählbyte und die Seite war ungültig. Für Browser
  (User-Agent Mozilla) enden die Kopfseiten mit einem `PADDING`-Kommentar knapp unter 32 kB und der Ton kommt mit
  konstanter Paketgröße (CBR): Chrome reicht einen Strom ohne Content-Length nur in vollen 32-kB-Blöcken an seinen
  Demuxer weiter (gemessen mit dem Stationsstrom in Echtzeit: Bursts bis 62 kB und 93 kB nichts, 68 kB und 99 kB spielen).
  Mit VBR-Stille (3-Byte-Ruhepakete, ~0,5 kB/s) brauchte ein leiser Kanal über eine Minute je Block – der Browser-Stream von
  DF0HHH blieb scheinbar für immer stumm. Jetzt füllen die ersten Tonseiten den ersten Block sofort, jeder weitere kommt
  bei 32 kbit/s nach ~7,5 s (`?br=64`: ~4 s); so viel Verzögerung hat der Browser-Stream, der WebSocket-Ton der Oberfläche
  nicht.
- **Ogg-Stream spielt jetzt auch im Browser:** Chrome legte für das eingebettete Stationsbild eine Videospur ohne Bilder an
  und blieb stumm. Browser (User-Agent Mozilla) bekommen den Stream ohne Bild, Abspielprogramme weiter mit; `?pic=0|1`
  erzwingt es. Beispiel-Adressen auf der Info-Seite nutzen die öffentliche Adresse der Station ([station] url).

### Info-Seite
- **Impressum und Datenschutzerklärung neu gefasst:** sachlich, ohne Anrede, auf das beschränkt, was die Software
  tatsächlich verarbeitet; Chat-Haltegrenzen, Verbund und Verzeichnis kommen aus der Konfiguration. Neu `[station]`
  `hosting` und `hosting_log_days` für einen vorgeschalteten Server (Proxy/Hoster) mit seinem Zugriffsprotokoll.

### Decoder
- **RADE: Rufzeichen am Ende der Durchgabe.** `crab_rade` liest jetzt das Rufzeichen aus dem EOO-Rahmen (Format der
  FreeDV-App: LDPC HRA_56_56 mit CRC-8) und meldet es als `{"kind":"text","mode":"RADE V1","text":"DO1XX","snr":…}`;
  das Ereignis `eoo` bleibt. Decoder-Modul `plugins/rade/eoo/EooCallsignCodec` aus
  [radae_decoder](https://github.com/peterbmarks/radae_decoder) (Peter Marks, LGPL 2.1, siehe THIRD-PARTY.md), per
  C-Brücke eingebunden – `build.sh` braucht jetzt auch einen C++-Compiler (`CXX`, im Cross-Container `$HOST-g++`).
  Zeichensatz wie in FreeDV: A–Z, 0–9 und `&'()*+,-./`, wobei `/` on air wie `0` kodiert wird (`DF0HHH/P` → `DF0HHH0P`).
- **RADE: sauberer Start je Durchgabe.** Die Sprachsynthese (FARGAN) beginnt bei jedem neuen Sync von vorn
  (`fargan_init` + 5 Rahmen Vorlauf, wie `webrx_rade_decode`), statt den Zustand der vorigen Durchgabe mitzunehmen.
- **RADE: Frequenzversatz im Sync-Ereignis.** `sync` trägt `foff` (Hz, aus `rade_freq_offset`) – zeigt, wie weit der
  Hörer neben der Sendefrequenz liegt (Mitschnitt DF0HHH: −18 Hz, passt zur Stick-Messung).
- Testsender `plugins/rade/rade_eoo_tx.c` (`EOO_TX=1 plugins/rade/build.sh`): Sprache 16 kHz → RADE-V1-Signal 8 kHz mit
  EOO-Rufzeichen, für Prüfungen des Decoders ohne Funkgerät.
- **Neuestes SSTV-Bild als feste Adresse:** `/digi/sstv/latest.png` (immer das jüngste Bild, nie gecacht), `/digi/sstv/latest.json`
  und die Vollbildseite `/digi/latest.html` für Monitore und Spiegel (`?decoder=`, `?max_age=`, `?caption=0`, `?every=`).
- **SSTV: Farben und Zeilen stimmen wieder.** Der Decoder bekam vom Lauscher den Puffer ohne die Lage des VIS-Kopfs
  und nahm den Stoppbit als ersten Zeilensync; alle Zeilen rutschten eine nach oben – bei Robot 36 vertauscht das
  R-Y/B-Y (grün/magenta), bei Martin verschiebt es die Farben. Jetzt wird das VIS-Ende mitgegeben.
  Prüfung gegen den Referenz-Encoder pysstv: `plugins/sstv/test_sstv.py` (Robot 36, Martin 1/2, Scottie 1, PD 120).

### Ton
- **Rauschsperre ohne Rauschfahne:** die Sperre schließt jetzt am ungeglätteten Pegel (bisher hielt der geglättete Pegel
  sie nach dem Trägerende noch 100–150 ms offen), und mit eingeschalteter Sperre läuft der Ton einen Rahmen (20 ms)
  verzögert – der Rahmen mit dem Trägerende und der erste Rahmen beim Öffnen werden verworfen, wie es Funkgeräte über
  Rauschdetektor und Abschaltton auch tun. Im Messaufbau (`tools/fake_rtl_tcp.py 6999 3,2`, getasteter Träger) bleibt
  nach fünf Trägerenden kein Rauschstoß mehr. Ohne Sperre keine Verzögerung.
- **FM klingt wie im Funkgerät:** De-Emphasis −6 dB/Oktave ab 300 Hz (530 µs, TIA-603) und Tiefpass 3,2 kHz statt 4 kHz.
  Funkgeräte senden mit Pre-Emphasis; ohne die Umkehr klang Sprache dünn und das Diskriminatorrauschen zischte.
  Lautstärke bleibt gleich (Ausgleich ×2,2). Betriebsart „Daten“ bleibt flach.

## 1.4.0 – 2026-10-11

Betriebsart DV versteht RADE (FreeDV ab 2.0); freier VFO für private Stationen.

### Freier VFO (Labor)
- `free_vfo = true` (Station oder Band, `free_vfo_min`/`free_vfo_max` als Grenzen): Hörer verschieben die Mitte des
  Wasserfalls im Betrieb – Zeile **Mitte** mit Eingabe in MHz und Schritten ±½ Band, ±1, ±10 MHz. Gilt für alle Hörer
  des Bandes, deshalb nur für private Stationen; Bänder ohne die Option lassen sich weiter nicht umstimmen
  (bisher ging das nur für Admins auf Admin-Bändern, ohne Bedienelement).
- Skala, Bandleiste, Bereiche und Schnellwahl folgen sofort, der Wasserfall beginnt neu; die eigene Frequenz bleibt
  stehen oder rutscht an den Rand des neuen Fensters. Link „Teilen“ enthält die Mitte (`&band=…&vfo=`).
- Grenzen nach Empfänger (RTL-Stick 24–1766 MHz, HackRF, Airspy) oder aus der Konfiguration; Konverter-Bänder ohne
  Treibergrenze. Feste Decoder auf einem Band mit freiem VFO bleiben aus (`--check` meldet es), DV-Decoder außerhalb
  des neuen Fensters enden sofort. Die neue Mitte gilt bis zum Neustart.

### Betriebsart DV: RADE (FreeDV ab 2.0)
- Neues Plugin **`rade`**: dekodiert RADE V1, die Wellenform der FreeDV-App ab Version 2.0 (neuronaler Sprachcodec,
  OFDM mit 30 Trägern) – das, was heute auf 144,280 MHz und auf Kurzwelle tatsächlich gefahren wird; die Codec-2-Modes
  des Plugins `freedv` sind on air kaum noch zu hören. Programm `crab_rade` (C, gegen die Bibliothek
  [rade_c](https://github.com/freedv/rade_c), BSD-2-Clause, Sprachsynthese FARGAN aus Opus): USB-Ton 8 kHz rein,
  Sprache 16 kHz raus, Sync- und EOO-Ereignisse mit SNR; braucht auf einem Pi 4 etwa 20 % eines Kerns je Hörerfrequenz.
  Bauen: `plugins/rade/build.sh` (holt rade_c und Opus in festen Ständen), statisch für amd64/arm64:
  `packaging/build-rade.sh`; Release-Archiv, Debian-Pakete und Docker-Image enthalten das Programm (nicht für ARMv7).
- **DV** startet auf der Hörerfrequenz jetzt alle vorhandenen DV-Plugins (`rade` und `freedv`) als Gruppe `dv-<Hz>`;
  der Hörer hört den Decoder, der gerade Lock hat (bei beiden: RADE), der Knopf leuchtet beim Lock eines der beiden.
  `dv_max` zählt weiter Frequenzen, nicht Prozesse. Fehlt `freedv` (kein libcodec2), läuft DV mit `rade` allein und
  umgekehrt; der DV-Knopf erscheint nur, wenn ein DV-Plugin wirklich lauffähig ist (Programm vorhanden).
- Sync-Ereignisse der Gruppe tragen die Gruppen-ID und das Plugin (`data.plugin`), Hörer-Kennungen `dv-<Hz>` bleiben
  gültig; Decoder-IDs der Mitglieder heißen `dv-<Hz>-rade` / `dv-<Hz>-freedv`.

### Oberfläche
- DV-Knopf ohne dauerhaften Akzentrand.
- DV-Knopf leuchtet jetzt wirklich bei Lock: die Antwort der Station auf `dv` (Kennung des Decoders) kam als Textnachricht
  und wurde nur als Binärrahmen ausgewertet, der Lock blieb deshalb unsichtbar.

## 1.3.2 – 2026-10-07

Betriebsart DV: FreeDV auf der Frequenz des Hörers.

### Oberfläche
- **DV** neben FM/AM/USB/LSB/CW: abstimmen wie gewohnt, DV drücken, die Station startet dort einen FreeDV-Decoder (Hörer auf derselben Frequenz teilen ihn) und spielt die dekodierte Sprache. DV bleibt beim Abstimmen an und folgt der Frequenz (kurze Verzögerung beim Drehen); erst ein anderer Betriebsarten-Knopf schaltet zurück. In DV leuchtet nur DV, Bandbreite und AGC sind ausgegraut; der Knopf pulsiert bei Lock.
- Flüchtige DV-Decoder erscheinen nicht im Decoder-Fenster; FreeDV-Text nur bei stabilem Lock.

### Server
- Decoder können zur Laufzeit entstehen (`dv_max`, Vorgabe 3 gleichzeitig je Station) und enden, wenn niemand mehr hört (sofort beim Weiterdrehen, sonst nach 90 s). Sync und Text von Ton-Decodern gehen live an alle Hörer des Bandes.

## 1.3.1 – 2026-10-07

### Oberfläche
- Betriebsart **DV**: Knopf neben FM/AM/USB. Der Hörer stimmt wie gewohnt ab und drückt DV, die Station startet auf dieser Frequenz einen FreeDV-Decoder (Hörer auf derselben Frequenz teilen ihn, Ende nach 90 s ohne Hörer, höchstens `dv_max` gleichzeitig) und spielt die dekodierte Sprache. DV bleibt beim Abstimmen an und folgt der Frequenz; erst ein anderer Betriebsarten-Knopf schaltet zurück. Abstimmen oder eine andere Betriebsart holt den normalen Kanal zurück. Der Knopf pulsiert, solange der Decoder Sync hat (Lock), die Station meldet Sync und Text live an alle Hörer. FreeDV steht nicht mehr auf der Digital-Seite.

## 1.3.0 – 2026-10-07

Zwei neue Decoder: FreeDV und Pager.

### Decoder
- **FreeDV** (Plugin `freedv`, libcodec2): Codec-2-Modes 700D, 700E und 1600 laufen parallel auf einem USB-Kanal; die dekodierte Sprache ist als Kanal hörbar (Knopf „Dekodiert hören“ im Decoder-Fenster, Digital-Seite, `?dec=<id>`), Rufzeichen aus dem Textkanal als Treffer, Sync und SNR je Mode. Braucht `libcodec2-1.2`.
- **Pager/POCSAG** (Plugin `pocsag`, multimon-ng): 512/1200/2400 Bd auf der Betriebsart `data`, Rufe mit RIC, Funktion und Text, Übersicht auf der Digital-Seite. Für DAPNET auf 439,9875 MHz.
- Decoder können jetzt Ton zurückliefern (`output` in `decoder.json`): crabSDR kodiert ihn als Opus, verteilt ihn an Hörer, die `listen_decoder` gewählt haben, und unter `/stream/decoder/<id>.ogg` (öffentliche Decoder frei, andere mit Sysop-Token oder Stream-Schlüssel).

## 1.2.2 – 2026-10-07

### Härtung
- WebSocket-Befehle: Bandbreite auf 100 Hz bis 250 kHz begrenzt (vorher beliebig, ein Megahertz-Kanal hätte den DSP ausgelastet), Frequenz außerhalb des Bandes wird mit Hinweis abgelehnt, Nachrichten höchstens 16 kB.

## 1.2.1 – 2026-10-07

### Stream
- Fester Stream-Schlüssel (`stream_key`, Admin-Seite → Station → „Erzeugen“): `?token=<Schlüssel>` zählt wie der Sysop, gilt nur für `/stream/…`, läuft nicht ab. Für I/Q-Streams und gesperrte Bänder in VLC, ohne Anmelde-Token aus dem Browser.

## 1.2.0 – 2026-10-07

### Stream
- I/Q-Stream `…/stream/<kHz>/iq.wav` (links I, rechts Q, 48 oder 32 kHz, 16 oder 8 bit) und als Experiment `iq.ogg` (Opus auf I/Q): komplexes Basisband eines Kanals für externe Decoder (TETRA, DMR). Freigabe über `iq_stream` (Vorgabe nur Sysop), höchstens `max_iq` (1) gleichzeitig. Siehe docs/STREAM.md.

## 1.1.2 – 2026-10-07

### Oberfläche
- Info-Seite: Abschnitte „Link mit Einstellungen“ und „Stream für VLC, mpv und Decoder“ mit Anleitung und anklickbaren Beispielen aus der Schnellwahl der Station.

## 1.1.1 – 2026-10-07

### Stream
- Ogg/Opus-Stream trägt Titel (Frequenz, Betriebsart, Station) und Künstler in den OpusTags; liegt `logo.png` oder `logo.jpg` im Stationsordner, wird es als Titelbild mitgeschickt (VLC zeigt beides).

## 1.1.0 – 2026-10-07

Stream in WAV und Stereo, Link mit allen Einstellungen, Lastgrenzen.

### Stream
- `.wav` zusätzlich zu `.ogg`: PCM 16 bit 48 kHz, verlustfrei für Decoder (DMR, POCSAG, AFSK).
- `pair.wav?l=<kHz>/<mode>&r=<kHz>/<mode>`: zwei Kanäle auf links und rechts, für ein Audiokabel mit zwei Decodern.
- `br`: Opus-Bitrate je Stream (8–128 kbit/s); Opus wird jetzt je Stream kodiert, der DSP liefert PCM.

### Oberfläche
- Teilen-Link trägt Rauschsperre und AGC; im Link außerdem `vol`, `mute`, `name` und `ui=min` (Minimal-Ansicht ohne Wasserfall für viele Tabs). Tab-Titel zeigt Frequenz und Betriebsart.
- Hinweis statt stummem Fehlschlag, wenn die Station voll ist.

### Lastgrenzen
- Neue Schlüssel `max_listeners` (50), `max_per_ip` (10), `max_channels` (16), `max_streams` (6) je Band, 0 = keine Grenze. Jede andere Frequenz/Betriebsart/Bandbreite kostet einen DSP-Kanal, Hörer auf demselben Kanal teilen ihn; darüber „Station voll“.

## 1.0.0 – 2026-10-07

Erste Veröffentlichung.

- Ein Programm, eine Konfigurationsdatei: mehrere Empfänger (RTL-SDR, SDRplay über SoapySDR, rtl_tcp), jeder Hörer mit eigener Frequenz und Betriebsart (FM, AM, SAM, USB, LSB, CW, WFM, Daten).
- Wasserfall mit Zoom bis in den Kanal, Bereiche nach Bandplan, Schnellwahl, Teilen-Link, Aufnahme.
- Ton als Opus mit anpassendem Vorlauf; AGC wie am Transceiver mit Schwelle, Rauschsperre mit sofortigem Schließen.
- Ton-Stream für VLC, mpv und Decoder (`/stream/<kHz>/<betriebsart>.ogg`, Senderliste `presets.m3u`).
- Decoder als Plugins (APRS mit Karte und Digipeater-Übersicht, FT8, SSTV), Treffer per MQTT.
- Chat und Logbuch der Hörer, Verbund-Chat über crabsdr.de, Hörerzahl nach Personen.
- Admin-Seite: Bänder, Decoder, Betreiberangaben, Konfiguration ohne Neustart, Verzeichnis-Anmeldung bei crabsdr.de.
- Oberfläche für Handy und als Web-App, Tastaturbedienung, Info-Seite mit Impressum und Datenschutz.
- Debian-Pakete (amd64, arm64, armhf), statische Binärdateien und Docker-Image; Frequenzkorrektur je Band in Software.
