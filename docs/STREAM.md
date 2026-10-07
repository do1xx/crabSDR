# Ton als Stream (Ogg/Opus über HTTP)

Jeder Kanal einer Station lässt sich wie ein Webradio abrufen. Das spielt VLC, mpv, jeder Browser, Home Assistant
oder ein Skript, ohne Oberfläche und ohne Klick.

```
https://<station>/stream/<Frequenz in kHz>/<Betriebsart>.ogg
```

Beispiele:

```
https://df0hhh.crabsdr.de/stream/145700/fm.ogg
https://df0hhh.crabsdr.de/stream/144260/usb.ogg?pb=300,2700&agc=slow
https://df0hhh.crabsdr.de/stream/145700/data.ogg?sq=off          # flacher FM-Ausgang für Decoder
```

Das Band sucht der Server anhand der Frequenz. Liegt sie in keinem Band, antwortet er mit 404.

## Betriebsarten

`fm`, `data`, `am`, `sam`, `usb`, `lsb`, `cw`, `wfm`. **`data`** ist FM mit flachem Diskriminator-Ausgang: kein
300-Hz-Hochpass, keine Sprachfilterung, bis 8 kHz, ausgelegt auf ±4,5 kHz Hub. Für POCSAG, AFSK, DTMF, ZVEI.

## Parameter

| Parameter | Werte | Vorgabe |
|---|---|---|
| `bw` | Bandbreite in Hz | FM 12500, AM 9000, SSB 2700, CW 500 |
| `pb` | SSB-Durchlass `lo,hi` in Hz (wie im Teilen-Link) | 300–3000 |
| `sq` | `auto` (6 dB über dem Rauschen), `auto:10` (eigener Abstand), `-85` (fest in dBFS), `off` | `auto` bei FM/Daten, sonst `off` |
| `agc` | `fast`, `medium`, `slow`, `off` | `medium` |
| `name` | Anzeigename in der Hörerliste | `Stream` |
| `band` | Band-ID, nur wenn zwei Bänder die Frequenz abdecken | – |
| `token` | Zugang (Gast-Token oder Anmeldung), falls das Band nicht für Gäste frei ist | – |

Bei geschlossener Rauschsperre wird Stille gesendet, nicht nichts, sonst beendet der Spieler die Verbindung.

## Senderliste

`https://<station>/stream/presets.m3u` enthält die Schnellwahl der Station. In VLC: Medien → Netzwerkstream öffnen,
oder die Datei herunterladen und doppelklicken.

## Rechte und Zählung

Der Stream unterliegt denselben Regeln wie der Browser: Bänder, die Gäste hören dürfen, sind ohne Token abrufbar,
alle anderen brauchen `?token=`. Jeder Stream zählt als Hörer und erscheint in der Hörerliste.

## Bandbreite

Opus mit der Bitrate der Station (Voreinstellung 32 kbit/s), rund 4 kB/s je Stream.

## Decoder anschließen

Jede Decoder-Software, die Ton von einer Soundkarte erwartet, bekommt ihn über ein virtuelles Audiokabel: ein Spieler
(VLC, mpv) spielt `…/data.ogg?sq=off` auf das Kabel, der Decoder hört darauf. Ohne Umweg über eine Soundkarte geht es
direkt in der Kommandozeile, zum Beispiel mit multimon-ng:

```
curl -sN "https://<station>/stream/145700/data.ogg?sq=off" | ffmpeg -loglevel quiet -i - -f s16le -ar 22050 -ac 1 - | multimon-ng -t raw -a POCSAG1200 -a AFSK1200 -
```
