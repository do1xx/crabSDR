//! `crabsdr-server --check [config.toml]`: Konfiguration prüfen, ohne den Server zu starten. Meldet Syntaxfehler mit
//! Zeile, unbekannte und alte Schlüssel, Bänder, Decoder (Plugin vorhanden? Programme installiert? Frequenz im Band?),
//! Pfade und Quellen. Rückgabe 0 = startklar (Hinweise erlaubt), 1 = Fehler. Die Installationsskripte rufen das vor
//! jedem Neustart auf, damit eine kaputte Konfiguration nie die laufende Station abschaltet.

use crabsdr_core::ServerConfig;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::Path;
use std::time::Duration;

pub fn run(path: Option<&Path>) -> i32 {
    println!("crabSDR {} – Prüfung der Konfiguration", crate::version_long());
    let loaded = match ServerConfig::load_checked(path) {
        Ok(l) => l,
        Err(e) => { println!("✗ {}", e); println!("\nErgebnis: FEHLER – so startet crabSDR nicht."); return 1; }
    };
    let c = &loaded.config;
    let (mut errors, mut warns) = c.validate();
    warns.splice(0..0, loaded.warnings.iter().cloned());
    println!("  Datei        {}", c.source.as_ref().map_or("(keine, Voreinstellungen)".into(), |p| p.display().to_string()));
    println!("  Station      {}{}", c.station.name, c.station.home().map_or(String::new(), |(a, b)| format!(" · {:.4} {:.4}", a, b)));
    println!("  Port         {}", c.port);

    // Pfade
    let dir = |label: &str, p: &Path, marker: Option<&str>, need: bool, errors: &mut Vec<String>, warns: &mut Vec<String>| {
        let ok = match marker { Some(m) => p.join(m).exists(), None => p.is_dir() };
        println!("  {:<12} {} {}", label, p.display(), if ok { "✓" } else { "✗ fehlt" });
        if !ok {
            let msg = format!("{} {} fehlt{}", label, p.display(), marker.map_or(String::new(), |m| format!(" (keine {})", m)));
            if need { errors.push(msg) } else { warns.push(msg) }
        }
    };
    dir("Oberfläche", &c.frontend_dir, Some("index.html"), true, &mut errors, &mut warns);
    if let Some(s) = &c.site_dir { dir("Stationsordner", s, None, false, &mut errors, &mut warns); }
    dir("Plugins", &c.plugin_dir, None, !c.decoders.is_empty(), &mut errors, &mut warns);
    dir("Daten", &c.data_dir, None, false, &mut errors, &mut warns);

    // Bänder
    println!("\nBänder ({}):", c.sdrs.len());
    for b in &c.sdrs {
        let (lo, hi) = (b.center_freq as f64 - b.sample_rate as f64 / 2.0, b.center_freq as f64 + b.sample_rate as f64 / 2.0);
        let src = if b.sdr_driver == "rtl_tcp" {
            let addr = format!("{}:{}", b.sdr_tcp_host, b.sdr_tcp_port);
            let up = addr.to_socket_addrs().ok().and_then(|mut a| a.next()).is_some_and(|a| TcpStream::connect_timeout(&a, Duration::from_millis(800)).is_ok());
            if !up && b.enabled { warns.push(format!("Band „{}“: rtl_tcp {} gerade nicht erreichbar", b.id, addr)); }
            format!("rtl_tcp {} {}", addr, if up { "✓" } else { "(nicht erreichbar)" })
        } else { format!("{} {}", b.sdr_driver, b.sdr_device) };
        println!("  {} {:<12} {:>10.4}–{:<10.4} MHz  gain {:<5} {}{}{}", if b.enabled { "•" } else { "–" }, b.id, lo / 1e6, hi / 1e6, b.gain, src,
            b.smeter_cal.map_or(String::new(), |v| format!("  S-Meter {:+.1} dB", v)), if b.enabled { "" } else { "  (aus)" });
    }

    // Decoder
    if !c.decoders.is_empty() {
        println!("\nDecoder ({}):", c.decoders.len());
        for d in &c.decoders {
            let id = d.id.clone().unwrap_or_else(|| format!("{}-{}", d.plugin, d.freq / 1000));
            let pdir = c.plugin_dir.join(&d.plugin);
            let man = std::fs::read_to_string(pdir.join("decoder.json")).ok().map(|s| serde_json::from_str::<crate::decoders::Manifest>(&s));
            let state = match man {
                None => { let m = format!("Decoder „{}“: Plugin „{}“ fehlt ({}/decoder.json)", id, d.plugin, pdir.display()); if d.enabled { errors.push(m) } else { warns.push(m) }; "✗ Plugin fehlt".to_string() }
                Some(Err(e)) => { errors.push(format!("Decoder „{}“: decoder.json fehlerhaft: {}", id, e)); "✗ decoder.json fehlerhaft".into() }
                Some(Ok(m)) => {
                    let miss: Vec<&String> = m.requires.iter().filter(|p| !crate::decoders::have_program(p, &pdir)).collect();
                    if miss.is_empty() { "✓".into() } else {
                        if d.enabled { warns.push(format!("Decoder „{}“: Programm fehlt: {} (Decoder bleibt aus)", id, miss.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "))); }
                        format!("fehlt: {}", miss.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "))
                    }
                }
            };
            println!("  {} {:<14} {:>9.4} MHz  {}{}{}", if d.enabled { "•" } else { "–" }, id, d.freq as f64 / 1e6, state,
                if d.public { "" } else { "  (nicht öffentlich)" }, if d.enabled { "" } else { "  (aus)" });
        }
    }
    if let Some(m) = &c.mqtt { println!("\nMQTT: {} {}:{} Thema {}", if m.enabled { "an" } else { "aus" }, m.host, m.port, m.topic); }

    if !warns.is_empty() { println!("\nHinweise:"); for w in &warns { println!("  ! {}", w); } }
    if !errors.is_empty() { println!("\nFehler:"); for e in &errors { println!("  ✗ {}", e); } }
    if errors.is_empty() { println!("\nErgebnis: in Ordnung{}", if warns.is_empty() { "" } else { " (mit Hinweisen)" }); 0 } else { println!("\nErgebnis: FEHLER"); 1 }
}
