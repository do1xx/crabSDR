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
| `name` | Anzeigename in der Hörerliste | `Stream` |
| `band` | Band-ID, nur wenn zwei Bänder die Frequenz abdecken | – |
| `rate`, `bits` | nur `iq.wav`: 32000/48000 Hz, 8/16 bit | 48000, 16 |
| `token` | Zugang: Gast-Token, Anmeldung oder der feste **Stream-Schlüssel** der Station (`stream_key`, Admin-Seite → Station; zählt wie der Sysop, nur für Streams, läuft nicht ab) | – |

Bei `pair.wav` gelten `bw`, `sq` und `agc` für beide Kanäle. Bei geschlossener Rauschsperre wird Stille gesendet,
nicht nichts, sonst beendet der Spieler die Verbindung.

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
