# Drittcode

crabSDR steht unter der GNU AGPL 3.0 oder neuer (siehe LICENSE). Folgende Teile stammen von anderen und behalten ihre Lizenz:

| Teil | Lizenz | Herkunft |
|---|---|---|
| `web/zstd.js` (fzstd 0.1.1) | MIT, Arjun Barrett | npm-Paket fzstd, unverändert |
| `web/fonts/jetbrains-mono.woff2` | SIL Open Font License 1.1 | JetBrains |
| `web/digi/vendor/maplibre-gl.js` | BSD-3-Clause | MapLibre |
| Rust-Abhängigkeiten (`backend-rs/Cargo.lock`) | MIT, Apache-2.0, ISC, BSD, Unicode u. a. | crates.io, alle permissiv |

Externe Programme, die crabSDR als eigene Prozesse aufruft (rtl_sdr, rx_sdr, direwolf, der FT8-Decoder aus WSJT-X, SoapyMiri,
libmirisdr), sind nicht Teil dieses Repos und stehen unter ihren eigenen Lizenzen (überwiegend GPL).
