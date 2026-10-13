# Ton als Stream und Link mit Einstellungen

## Stream (Ogg/Opus oder WAV über HTTP)

Jeder Kanal einer Station lässt sich wie ein Webradio abrufen. Das spielt VLC, mpv, jeder Browser, Home Assistant
oder ein Decoder, ohne Oberfläche und ohne Klick.

```
https://<station>/stream/<Frequenz in kHz>/<Betriebsart>.ogg     Opus, klein (32 kbit/s), fürs Ohr
https://<station>/stream/<Frequenz in kHz>/<Betriebsart>.wav     PCM 16 bit 48 kHz, verlustfrei (770 kbit/s), für Decoder
https://<station>/stream/pair.wav?l=<kHz>/<mode>&r=<kHz>/<mode>  zwei Kanäle auf links und rechts (Audiokabel mit zwei Decodern)
https://<station>/stream/presets.m3u                             Schnellwahl der Station als Senderliste
```

Beispiele:

```
https://df0hhh.crabsdr.de/stream/145700/fm.ogg
https://df0hhh.crabsdr.de/stream/144260/usb.ogg?pb=300,2700&agc=slow&br=64
https://df0hhh.crabsdr.de/stream/144800/data.wav?sq=off
https://df0hhh.crabsdr.de/stream/pair.wav?l=145700/fm&r=145725/fm&sq=off
```

Das Band sucht der Server anhand der Frequenz. Liegt sie in keinem Band, antwortet er mit 404.

### Betriebsarten

`fm`, `data`, `am`, `sam`, `usb`, `lsb`, `cw`, `wfm`. **`data`** ist FM mit flachem Diskriminator-Ausgang: kein
300-Hz-Hochpass, keine Sprachfilterung, bis 8 kHz, ausgelegt auf ±4,5 kHz Hub. Für POCSAG, AFSK, DTMF, ZVEI; mit `.wav`
auch für 4FSK (DMR) – Opus rundet die Flanken ab, PCM nicht.

### Parameter

| Parameter | Werte | Vorgabe |
|---|---|---|
| `bw` | Bandbreite in Hz | FM 12500, AM 9000, SSB 2700, CW 500 |
| `pb` | SSB-Durchlass `lo,hi` in Hz (wie im Teilen-Link) | 300–3000 |
| `sq` | `auto` (6 dB über dem Rauschen), `auto:10` (eigener Abstand), `-85` (fest in dBFS), `off` | `auto` bei FM/Daten, sonst `off` |
| `agc` | `fast`, `medium`, `slow`, `off` | `medium` |
| `br` | Opus-Bitrate in kbit/s, 8–128 (nur `.ogg`) | Bitrate der Station (32) |
| `pic` | `1` = Stationsbild in den OpusTags, `0` = ohne. Ohne Angabe: mit für Abspielprogramme, ohne für Browser (User-Agent Mozilla) – Chrome spielt Ogg mit eingebettetem Bild nicht ab |
| `live` | `1` = Live-Titel per Ogg-Kettung (siehe unten), `0` = ein fester Titel. Ohne Angabe: an für Abspielprogramme, aus für Browser |
| `cover` | `wf` = alle 10 s der Wasserfall um die Frequenz als Titelbild (nur mit Live-Titeln, nur `.ogg`) |
| `name` | Anzeigename in der Hörerliste | `Stream` |
| `band` | Band-ID, nur wenn zwei Bänder die Frequenz abdecken | – |
| `rate`, `bits` | nur `iq.wav`: 32000/48000 Hz, 8/16 bit | 48000, 16 |
| `token` | Zugang: Gast-Token, Anmeldung oder der feste **Stream-Schlüssel** der Station (`stream_key`, Admin-Seite → Station; zählt wie der Sysop, nur für Streams, läuft nicht ab) | – |

Bei `pair.wav` gelten `bw`, `sq` und `agc` für beide Kanäle. Bei geschlossener Rauschsperre wird Stille gesendet,
nicht nichts, sonst beendet der Spieler die Verbindung.

### Live-Titel: Pegel, Rauschsperre und Rufzeichen im Abspieler

Der Ogg-Stream trägt für Abspielprogramme einen **lebenden Titel**: VLC, mpv und alles, was die Tags eines Webradios
anzeigt (Auto-Radio über Bluetooth, Now-Playing-Anzeige des Handys), sehen statt eines festen Textes den aktuellen Zustand
des Kanals.

```
145700 kHz FM · +18 dB              Rauschsperre offen, Signal 18 dB über dem Rauschen (6-dB-Stufen, 2 dB Hysterese)
145700 kHz FM · Rauschsperre zu
145700 kHz FM · DO1XX · +24 dB      Rufzeichen aus dem Decoder: RADE/FreeDV (Ende der Durchgabe, ±3 kHz, 10 min) oder
144800 kHz DATA · DB0XYZ-10 · +12 dB APRS (Absender des letzten Pakets, ±10 kHz, 90 s)
```

ARTIST ist `<Station> · crabSDR`, ALBUM der Stationsname. Technik: der Server kettet Ogg-Ströme (RFC 3533 „chained“, wie
Icecast beim Titelwechsel) – der laufende logische Strom endet mit einer EOS-Seite, ein neuer beginnt mit neuer
Seriennummer, Granule 0, OpusHead und OpusTags. Der Titel wird mit jeder Pegel-Meldung (4× je s) geprüft und höchstens
alle 5 s gewechselt; ohne Änderung bleibt der Strom ungekettet. Abspieler, die bei Kettung stolpern, bekommen mit
`?live=0` den alten festen Titel; Browser (User-Agent Mozilla) nie eine Kettung, denn Chrome legt für jeden neuen Strom
neue Spuren an.

An der Naht geht kein Abtastwert verloren und nichts knackt: der alte Opus-Kodierer gibt mit einem letzten Rahmen seine
Verzögerung (312 Werte) aus, der neue wird mit dem Rahmen vor der Naht angewärmt (Paket verworfen), sodass sein
Pre-Skip genau die schon gesendeten Werte deckt, und sein erster Rahmen ist ohne Vorhersage kodiert (intra) – ein
frischer Dekodierer (VLC, opusfile) wie ein weiterlaufender (ffmpeg, mpv) setzt nahtlos fort. Gemessen am 50-Hz-Ton
(`cargo test kette_sinus` mit `CRAB_CHAIN_OUT=<datei>`): Fehler an der Naht ≤ −33 dB, wie das Codec-Rauschen sonst.

| Abspieler | Titel | Cover | Ton an der Naht |
|---|---|---|---|
| VLC 3 (Mac/Windows/Linux/Android/iOS) | wechselt live, auch an Bluetooth/AVRCP und die Sperrbildschirm-Anzeige | ja (Titelbild) | kurzer Aussetzer (ein Rahmen, 20 ms): VLC 3 legt die Spur neu an, statt sie weiterzuverwenden (toter Zweig im Ogg-Demuxer, in VLC 4 behoben) |
| mpv | wechselt live (`--display-tags`, OSD, Terminal-Zeile „Title“) | nein | lückenlos („Linearizing discontinuity“ im Log) |
| ffmpeg/ffplay | wechselt live (Metadaten-Ereignis) | nein | lückenlos; beim Mitschnitt in eine Datei „Non-monotonic DTS“-Hinweise |
| opusdec/opusfile | – | – | lückenlos |
| Browser `<audio>` | fester Titel (`live` aus) | – | – |
| Auto-Radio über Bluetooth (VLC am Handy) | Titel/Interpret/Album als Text | je nach Radio | wie VLC |

**Wasserfall als Titelbild** (`?cover=wf`): alle 10 s (das erste nach 5 s) kommt im neuen Kettenglied ein JPEG 320 × 180 als
`METADATA_BLOCK_PICTURE` – die letzten 18 s Wasserfall eines Ausschnitts von etwa 250–500 kHz um die Frequenz des Hörers
(Zoomstufe als Zweierpotenz der Abtastrate, am Bandrand angeschlagen), neueste Zeile oben, Farben wie im Browser, der
gehörte Kanal mit rotem Balken und Kantenlinien markiert. Das Bild liegt bei 3–25 kB (JPEG, Qualität 60), Rechenzeit
unter 1 ms je Bild auf dem Mac (Release-Build), auf dem Pi 4 geschätzt 10–20 ms alle 10 s, also unter 0,2 % eines Kerns;
die Wasserfall-Zeilen kommen aus dem Band wie für einen Browser-Hörer (10 je s, zstd). Jedes Kettenglied wiederholt das
jeweils letzte Bild, damit es beim Titelwechsel nicht verschwindet.

```
https://df0hhh.crabsdr.de/stream/145700/fm.ogg?cover=wf     VLC: Titel lebt, Cover = Wasserfall
https://df0hhh.crabsdr.de/stream/145700/fm.ogg?live=0       fester Titel, keine Kettung
```

### Rechte, Zählung, Lastgrenzen

Der Stream unterliegt denselben Regeln wie der Browser: Bänder, die Gäste hören dürfen, sind ohne Token abrufbar,
alle anderen brauchen `?token=`. Jeder Stream zählt als Hörer und erscheint in der Hörerliste.

Damit niemand eine Station leersaugt, gelten je Band die Grenzen aus der Konfiguration (docs/CONFIG.md):
`max_streams` (Vorgabe 6), `max_per_ip` (10 Verbindungen je Adresse), `max_listeners` (50) und `max_channels`
(16 verschiedene Frequenz/Betriebsart/Bandbreite-Kombinationen; Hörer auf demselben Kanal teilen ihn). Darüber
antwortet der Server mit 503 bzw. 429 und einem Satz, was los ist.

### Dekodierter Ton eines Decoders

```
https://<station>/stream/decoder/<decoder-id>.ogg      z. B. …/stream/decoder/freedv-144350.ogg
```

Decoder, die Sprache zurückliefern (FreeDV), gibt es als eigenen Stream; die Decoder-ID steht auf der Digital-Seite und in
`/api/decoders`. Öffentliche Decoder sind frei, andere brauchen das Sysop-Token oder den Stream-Schlüssel.

### I/Q-Stream (komplexes Basisband, für TETRA, DMR und alles, was einen SDR-Decoder braucht)

```
https://<station>/stream/<kHz>/iq.wav                 links I, rechts Q, 48 kHz 16 bit (1,5 Mbit/s)
https://<station>/stream/<kHz>/iq.wav?rate=32000&bits=8   32 kHz, 8 bit (0,5 Mbit/s), reicht für einen 25-kHz-Kanal
https://<station>/stream/<kHz>/iq.ogg?br=192          Opus auf I/Q, Experiment (0,2 Mbit/s): Opus ist für Ohren gebaut,
                                                      ob ein Decoder damit klarkommt, muss der Versuch zeigen
```

Kein Demodulator: der Kanalausschnitt wird zur Mitte gemischt (Gleichanteil bleibt), auf die Abtastrate umgerechnet und
auf Spitze −6 dBFS geregelt. `bw` ist die Breite des Ausschnitts (Vorgabe 30000 Hz; TETRA braucht 25 kHz, DMR 12,5 kHz).
Weil ein I/Q-Hörer so viel Upload braucht wie 50 Ton-Hörer, gibt es ihn nur nach Freigabe: `iq_stream = "admin"` (Vorgabe,
nur der Sysop mit seinem Token), `"users"`, `"all"` oder `"off"`, und höchstens `max_iq` (Vorgabe 1) gleichzeitig.

```
curl -sN "https://<station>/stream/395000/iq.wav?bw=30000&token=…" | sox -t wav - -t raw -r 36000 -e float -b 32 -c 2 - | <tetra-decoder>
```

### Decoder anschließen

Jede Decoder-Software, die Ton von einer Soundkarte erwartet, bekommt ihn über ein virtuelles Audiokabel: ein Spieler
(VLC, mpv) spielt `…/data.wav?sq=off` auf das Kabel, der Decoder hört darauf; `pair.wav` legt zwei Frequenzen auf links
und rechts. Ohne Umweg über eine Soundkarte geht es direkt in der Kommandozeile, zum Beispiel mit multimon-ng:

```
curl -sN "https://<station>/stream/145700/data.wav?sq=off" | sox -t wav - -t raw -r 22050 -e signed -b 16 -c 1 - | multimon-ng -t raw -a POCSAG1200 -a AFSK1200 -
```

## Link mit Einstellungen (Browser)

Der Teilen-Link der Oberfläche trägt Frequenz, Betriebsart, Bandbreite, Rauschsperre und AGC:

```
https://<station>/?tune=145700.000fm&pb=-8.00,8.00&sq=auto:6
```

Weitere Parameter, von Hand anzuhängen:

| Parameter | Wirkung |
|---|---|
| `sq=auto:8`, `sq=off`, `sq=-85` | Rauschsperre wie beim Stream |
| `agc=slow` | AGC-Geschwindigkeit |
| `vol=-10` | Lautstärke in dB (−20 … 6) |
| `mute=1` | stumm starten |
| `name=DO1XX` | Anzeigename |
| `ui=min` | Minimal-Ansicht: nur Frequenz, Betriebsart, Bandbreite, Pegel; kein Wasserfall. Für viele Tabs nebeneinander |
| `view=all` | alle Bänder untereinander |

Der Tab-Titel zeigt Frequenz und Betriebsart. Ton startet in jedem Tab erst nach einem Klick, das ist eine Regel der
Browser, nicht von crabSDR.
