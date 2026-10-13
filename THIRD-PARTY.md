# Drittcode

crabSDR steht unter der GNU AGPL 3.0 oder neuer (siehe LICENSE). Folgende Teile stammen von anderen und behalten ihre Lizenz:

| Teil | Lizenz | Herkunft |
|---|---|---|
| `web/zstd.js` (fzstd 0.1.1) | MIT, Arjun Barrett | npm-Paket fzstd, unverändert |
| `web/fonts/jetbrains-mono.woff2` | SIL Open Font License 1.1 | JetBrains |
| `web/digi/vendor/maplibre-gl.js` | BSD-3-Clause | MapLibre |
| Rust-Abhängigkeiten (`backend-rs/Cargo.lock`) | MIT, Apache-2.0, ISC, BSD, Unicode u. a. | crates.io, alle permissiv |
| `plugins/rade/crab_rade` (gebaut aus `plugins/rade/crab_rade.c` + rade_c) | BSD-2-Clause (rade_c, David Rowe und FreeDV-Team) · BSD-3-Clause (Opus mit FARGAN, Xiph.Org) | github.com/freedv/rade_c, github.com/xiph/opus – Stände in `plugins/rade/build.sh`; das Programm enthält die Modellgewichte beider Projekte |
| `plugins/rade/eoo/EooCallsignCodec.{h,cpp}` | LGPL 2.1 (abgeleitet aus codec2, David Rowe u. a., und FreeDV-GUI, Mooneer Salem u. a.) | github.com/peterbmarks/radae_decoder `src/eoo/`, Stand baff453 (2026-09-25), unverändert; dekodiert das Rufzeichen aus dem RADE-EOO-Rahmen (LDPC HRA_56_56, CRC-8). Die C-Brücke `eoo_bridge.{h,cpp}` ist crabSDR (AGPL) |

Externe Programme, die crabSDR als eigene Prozesse aufruft (rtl_sdr, rx_sdr, direwolf, der FT8-Decoder aus WSJT-X, SoapyMiri,
libmirisdr), sind nicht Teil dieses Repos und stehen unter ihren eigenen Lizenzen (überwiegend GPL).
