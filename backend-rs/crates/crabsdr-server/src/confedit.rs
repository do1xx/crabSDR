//! Konfiguration über die Admin-Seite ändern (docs/SECURITY.md, Abschnitt Konfiguration).
//!
//! - Formulare schicken Änderungen ([`Op`]) nur für eine feste Liste von Einstellungen; die Textansicht schickt die
//!   ganze Datei. In beiden Fällen gilt: Pfade, Port und Schlüsselquellen ([`LOCKED`]) bleiben, wie sie sind.
//! - Kommentare, Reihenfolge und Schreibweise bleiben erhalten (toml_edit).
//! - Gespeichert wird nur, was die volle Prüfung besteht; vorher Sicherung, dann atomar ersetzt. Hat sich die Datei
//!   seit dem Lesen geändert (Prüfsumme), wird nichts überschrieben.

use crabsdr_core::ServerConfig;
use serde::Deserialize;
use serde_json::Value as J;
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::{Path, PathBuf};
use toml_edit::{value, DocumentMut, Item, Table};

pub const MAX_BYTES: usize = 512 * 1024;
pub const KEEP_BACKUPS: usize = 20;

/// Über die Admin-Seite nicht änderbar (nur in der Datei am Rechner)
pub const LOCKED: &[&str] = &["port", "frontend_dir", "plugin_dir", "data_dir", "db_path", "site_dir", "jwt_secret_env", "site", "mqtt.password_file"];

/// Decoder-Optionen mit Dateipfaden: ebenfalls nur in der Datei am Rechner (sonst könnte ein Admin-Konto Dateien der
/// Station überschreiben oder z. B. über igate_passfile eine geheime Datei an einen fremden Server schicken lassen)
pub const PATH_OPTIONS: &[&str] = &["logdir", "json", "log", "out", "igate_passfile", "passfile", "file", "path", "dir"];

const STATION_KEYS: &[&str] = &["name", "subtitle", "locator", "lat", "lon", "url", "operator", "address", "contact"];
/// Verzeichnis: nur Ein/Aus über die Admin-Seite; der Server steht in der Datei
const DIRECTORY_KEYS: &[&str] = &["enabled"];
const UI_KEYS: &[&str] = &["login", "chat", "logbook", "digital", "info", "recording", "status", "decoders", "banner", "impressum", "datenschutz", "admin"];
const TOP_KEYS: &[&str] = &["builtin_chat"];
const BAND_KEYS: &[&str] = &["id", "label", "note", "driver", "device", "host", "port", "center_freq", "sample_rate", "gain", "ppm", "mode",
    "enabled", "guest", "admin_only", "bias_tee", "smeter_cal", "fft_size", "fft_fps", "format", "settings"];
const DECODER_KEYS: &[&str] = &["plugin", "freq", "id", "band", "label", "enabled", "public", "options"];
/// neuer Name → alter Name (steht der alte in der Datei, wird er geändert)
const BAND_ALIASES: &[(&str, &str)] = &[("driver", "sdr_driver"), ("device", "sdr_device"), ("host", "sdr_tcp_host"), ("port", "sdr_tcp_port"), ("mode", "default_mode")];

#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
pub enum Seg { Index(usize), Key(String) }

#[derive(Debug, Deserialize, Clone)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum Op {
    /// Wert setzen; `null` entfernt den Schlüssel (Voreinstellung gilt wieder)
    Set { path: Vec<Seg>, value: J },
    /// Band oder Decoder entfernen: ["bands", i] / ["decoders", i]; Decoder-Option: ["decoders", i, "options", name]
    Remove { path: Vec<Seg> },
    /// Band oder Decoder anhängen: array = "bands" | "decoders"
    Append { array: String, table: serde_json::Map<String, J> },
}

pub fn digest(text: &str) -> String {
    Sha256::digest(text.as_bytes()).iter().map(|b| format!("{:02x}", b)).collect()
}

fn bad<T>(m: impl Into<String>) -> Result<T, String> { Err(m.into()) }

fn key(s: &Seg) -> Result<&str, String> { match s { Seg::Key(k) => Ok(k), Seg::Index(_) => bad("Pfad: Name erwartet") } }
fn idx(s: &Seg) -> Result<usize, String> { match s { Seg::Index(i) => Ok(*i), Seg::Key(_) => bad("Pfad: Nummer erwartet") } }

fn option_name_ok(k: &str) -> bool { !k.is_empty() && k.len() <= 40 && k.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') }
fn path_option(k: &str) -> bool { PATH_OPTIONS.contains(&k) || k.ends_with("_file") || k.ends_with("_dir") || k.ends_with("_path") }

/// JSON-Wert → TOML-Wert (nur einfache Werte)
fn scalar(v: &J) -> Result<toml_edit::Value, String> {
    Ok(match v {
        J::Bool(b) => (*b).into(),
        J::Number(n) if n.is_i64() => n.as_i64().unwrap().into(),
        J::Number(n) if n.is_u64() => i64::try_from(n.as_u64().unwrap()).map_err(|_| "Zahl zu groß".to_string())?.into(),
        J::Number(n) => n.as_f64().ok_or("Zahl")?.into(),
        J::String(s) => { if s.len() > 2000 { return bad("Text zu lang"); } s.as_str().into() }
        _ => return bad("nur Text, Zahl oder ja/nein"),
    })
}

/// Name der Band-Liste in der Datei ([[bands]] oder alt [[sdrs]])
fn band_array_name(doc: &DocumentMut) -> &'static str {
    if doc.get("bands").is_some() { "bands" } else if doc.get("sdrs").is_some() { "sdrs" } else { "bands" }
}

fn array_mut<'a>(doc: &'a mut DocumentMut, name: &str) -> Result<&'a mut toml_edit::ArrayOfTables, String> {
    let real = if name == "bands" { band_array_name(doc) } else { "decoders" };
    doc.get_mut(real).and_then(|i| i.as_array_of_tables_mut()).ok_or_else(|| format!("keine Liste [[{}]] in der Datei", real))
}

/// Schlüssel im Band: vorhandenen alten Namen weiterverwenden
fn band_key<'a>(t: &Table, k: &'a str) -> &'a str {
    for (new, old) in BAND_ALIASES { if *new == k && t.contains_key(old) && !t.contains_key(new) { return old; } }
    k
}

/// Wert setzen; Kommentar und Abstände des bisherigen Werts bleiben (z. B. „gain = 12   # Stufe“)
fn set_in(t: &mut Table, k: &str, v: &J) -> Result<(), String> {
    if v.is_null() { t.remove(k); return Ok(()); }
    let mut nv = scalar(v)?;
    if let Some(old) = t.get(k).and_then(|i| i.as_value()) { *nv.decor_mut() = old.decor().clone(); }
    t[k] = Item::Value(nv);
    Ok(())
}

fn table_of<'a>(doc: &'a mut DocumentMut, name: &str) -> &'a mut Table {
    if doc.get(name).and_then(|i| i.as_table()).is_none() {
        let mut t = Table::new(); t.set_implicit(false);
        doc[name] = Item::Table(t);
    }
    doc[name].as_table_mut().unwrap()
}

fn options_table(v: &J) -> Result<toml_edit::InlineTable, String> {
    let m = v.as_object().ok_or("options: Liste aus Name = Text")?;
    let mut it = toml_edit::InlineTable::new();
    for (k, x) in m {
        if !option_name_ok(k) { return bad(format!("Option „{}“: nur a–z, 0–9, _", k)); }
        if path_option(k) { return bad(format!("Option „{}“ ist ein Pfad – nur in der Datei am Rechner", k)); }
        let s = match x { J::String(s) => s.clone(), J::Number(n) => n.to_string(), J::Bool(b) => b.to_string(), _ => return bad("Option: Text erwartet") };
        it.insert(k, s.as_str().into());
    }
    Ok(it)
}

/// Änderungen anwenden (nur freigegebene Pfade). Ergebnis ist Text; ob er gilt, prüft [`check`].
pub fn apply(text: &str, ops: &[Op]) -> Result<String, String> {
    let mut doc: DocumentMut = text.parse().map_err(|e| format!("Datei nicht lesbar: {}", e))?;
    for op in ops {
        match op {
            Op::Set { path, value: v } => match path.as_slice() {
                [s0, k] if key(s0)? == "station" && STATION_KEYS.contains(&key(k)?) => set_in(table_of(&mut doc, "station"), key(k)?, v)?,
                [s0, k] if key(s0)? == "ui" && UI_KEYS.contains(&key(k)?) => set_in(table_of(&mut doc, "ui"), key(k)?, v)?,
                [s0, k] if key(s0)? == "directory" && DIRECTORY_KEYS.contains(&key(k)?) => set_in(table_of(&mut doc, "directory"), key(k)?, v)?,
                [k] if TOP_KEYS.contains(&key(k)?) => { let k = key(k)?; if v.is_null() { doc.remove(k); } else { doc[k] = value(scalar(v)?); } }
                [s0, i, k] if key(s0)? == "bands" && BAND_KEYS.contains(&key(k)?) => {
                    let arr = array_mut(&mut doc, "bands")?;
                    let t = arr.get_mut(idx(i)?).ok_or("Band gibt es nicht")?;
                    let real = band_key(t, key(k)?).to_string();
                    set_in(t, &real, v)?;
                }
                [s0, i, k] if key(s0)? == "decoders" && DECODER_KEYS.contains(&key(k)?) && key(k)? != "options" => {
                    let t = array_mut(&mut doc, "decoders")?.get_mut(idx(i)?).ok_or("Decoder gibt es nicht")?;
                    set_in(t, key(k)?, v)?;
                }
                [s0, i, o, name] if key(s0)? == "decoders" && key(o)? == "options" => {
                    let name = key(name)?; if !option_name_ok(name) { return bad("Optionsname: nur a–z, 0–9, _"); }
                    if path_option(name) { return bad(format!("Option „{}“ ist ein Pfad – nur in der Datei am Rechner", name)); }
                    let t = array_mut(&mut doc, "decoders")?.get_mut(idx(i)?).ok_or("Decoder gibt es nicht")?;
                    if t.get("options").and_then(|x| x.as_inline_table()).is_none() && t.get("options").and_then(|x| x.as_table()).is_none() {
                        t["options"] = value(toml_edit::InlineTable::new());
                    }
                    let s = match v { J::Null => None, J::String(s) => Some(s.clone()), J::Number(n) => Some(n.to_string()), J::Bool(b) => Some(b.to_string()), _ => return bad("Option: Text erwartet") };
                    if let Some(it) = t["options"].as_inline_table_mut() { match s { Some(s) => { it.insert(name, s.as_str().into()); } None => { it.remove(name); } } }
                    else if let Some(tt) = t["options"].as_table_mut() { match s { Some(s) => { tt[name] = value(s); } None => { tt.remove(name); } } }
                }
                _ => return bad(format!("Einstellung {:?} ist über die Admin-Seite nicht änderbar", path_str(path))),
            },
            Op::Remove { path } => match path.as_slice() {
                [s0, i] if key(s0)? == "bands" || key(s0)? == "decoders" => {
                    let arr = array_mut(&mut doc, key(s0)?)?; let i = idx(i)?;
                    if i >= arr.len() { return bad("gibt es nicht"); }
                    arr.remove(i);
                }
                [s0, i, o, name] if key(s0)? == "decoders" && key(o)? == "options" => {
                    if path_option(key(name)?) { return bad(format!("Option „{}“ ist ein Pfad – nur in der Datei am Rechner", key(name)?)); }
                    let t = array_mut(&mut doc, "decoders")?.get_mut(idx(i)?).ok_or("Decoder gibt es nicht")?;
                    if let Some(it) = t.get_mut("options").and_then(|x| x.as_inline_table_mut()) { it.remove(key(name)?); }
                }
                _ => return bad(format!("{} lässt sich nicht entfernen", path_str(path))),
            },
            Op::Append { array, table } => {
                let allowed = match array.as_str() { "bands" => BAND_KEYS, "decoders" => DECODER_KEYS, _ => return bad("nur bands oder decoders") };
                let mut t = Table::new();
                for (k, v) in table {
                    if !allowed.contains(&k.as_str()) { return bad(format!("„{}“ ist hier nicht erlaubt", k)); }
                    if k == "options" { t[k.as_str()] = value(options_table(v)?); } else if !v.is_null() { t[k.as_str()] = value(scalar(v)?); }
                }
                let name = if array == "bands" { band_array_name(&doc) } else { "decoders" };
                if doc.get(name).is_none() { doc[name] = Item::ArrayOfTables(toml_edit::ArrayOfTables::new()); }
                doc[name].as_array_of_tables_mut().ok_or("Liste in der Datei hat eine andere Form")?.push(t);
            }
        }
    }
    Ok(doc.to_string())
}

fn path_str(p: &[Seg]) -> String {
    p.iter().map(|s| match s { Seg::Key(k) => k.clone(), Seg::Index(i) => i.to_string() }).collect::<Vec<_>>().join(".")
}

/// Wert eines gesperrten Schlüssels (für den Vergleich alt/neu)
fn locked_values(text: &str) -> Vec<(String, Option<toml::Value>)> {
    let v: toml::Value = toml::from_str(text).unwrap_or(toml::Value::Table(Default::default()));
    LOCKED.iter().map(|k| {
        let mut cur = Some(&v);
        for part in k.split('.') { cur = cur.and_then(|c| c.get(part)); }
        (k.to_string(), cur.cloned())
    }).collect()
}

/// Pfad-Optionen aller Decoder (Plugin, Option, Wert), sortiert – für den Vergleich alt/neu
fn path_option_values(text: &str) -> Vec<(String, String, String)> {
    let v: toml::Value = toml::from_str(text).unwrap_or(toml::Value::Table(Default::default()));
    let mut out = Vec::new();
    for d in v.get("decoders").and_then(|d| d.as_array()).into_iter().flatten() {
        let plugin = d.get("plugin").and_then(|p| p.as_str()).unwrap_or("").to_string();
        for (k, x) in d.get("options").and_then(|o| o.as_table()).into_iter().flatten() {
            if path_option(k) { out.push((plugin.clone(), k.clone(), x.to_string())); }
        }
    }
    out.sort();
    out
}

/// Gesperrte Schlüssel, die sich geändert hätten. Pfad-Optionen der Decoder dürfen wegfallen, aber nicht neu
/// entstehen oder sich ändern.
pub fn locked_changes(old: &str, new: &str) -> Vec<String> {
    let mut v: Vec<String> = locked_values(old).into_iter().zip(locked_values(new)).filter(|(a, b)| a.1 != b.1).map(|(a, _)| a.0).collect();
    let before = path_option_values(old);
    for (plugin, k, val) in path_option_values(new) {
        if !before.iter().any(|(p, kk, vv)| *p == plugin && *kk == k && *vv == val) { v.push(format!("decoders.options.{} ({})", k, plugin)); }
    }
    v
}

#[derive(Debug, Default, serde::Serialize)]
pub struct CheckResult { pub errors: Vec<String>, pub warnings: Vec<String> }

/// Volle Prüfung eines Textes gegen die laufende Datei `current` (für gesperrte Schlüssel)
pub fn check(path: &Path, current: &str, text: &str) -> CheckResult {
    let mut r = CheckResult::default();
    if text.len() > MAX_BYTES { r.errors.push(format!("Datei größer als {} KiB", MAX_BYTES / 1024)); return r; }
    for k in locked_changes(current, text) {
        r.errors.push(format!("„{}“ ist über die Admin-Seite nicht änderbar (nur in der Datei am Rechner)", k));
    }
    match ServerConfig::from_content(text, path) {
        Err(e) => r.errors.push(e),
        Ok(l) => {
            r.warnings.extend(l.warnings);
            let (e, w) = l.config.validate();
            r.errors.extend(e); r.warnings.extend(w);
            // Decoder-Plugins müssen installiert sein
            for d in l.config.decoders.iter().filter(|d| d.enabled) {
                if !l.config.plugin_dir.join(&d.plugin).join("decoder.json").exists() {
                    r.errors.push(format!("Decoder-Plugin „{}“ ist nicht installiert ({})", d.plugin, l.config.plugin_dir.display()));
                }
            }
        }
    }
    r
}

#[derive(Debug)]
pub enum SaveError {
    /// Datei hat sich seit dem Lesen geändert
    Conflict,
    Invalid(CheckResult),
    ReadOnly(String),
    Io(String),
}

/// Speichern: Prüfsumme vergleichen, prüfen, sichern, atomar ersetzen. Gibt die neue Prüfsumme zurück.
pub fn save(path: &Path, backup_dir: &Path, expected: &str, text: &str) -> Result<(String, CheckResult), SaveError> {
    let current = std::fs::read_to_string(path).map_err(|e| SaveError::Io(format!("{}: {}", path.display(), e)))?;
    if digest(&current) != expected { return Err(SaveError::Conflict); }
    let res = check(path, &current, text);
    if !res.errors.is_empty() { return Err(SaveError::Invalid(res)); }
    if text == current { return Ok((digest(text), res)); }
    let dir = path.parent().unwrap_or(Path::new("."));
    // Sicherung (die letzten KEEP_BACKUPS)
    std::fs::create_dir_all(backup_dir).map_err(|e| SaveError::Io(format!("Sicherung {}: {}", backup_dir.display(), e)))?;
    let bak = backup_dir.join(format!("config-{}.toml", stamp()));
    std::fs::write(&bak, &current).map_err(|e| SaveError::Io(format!("Sicherung {}: {}", bak.display(), e)))?;
    prune_backups(backup_dir);
    // neue Datei daneben schreiben, Rechte übernehmen, dann ersetzen
    let tmp = dir.join(format!(".{}.neu-{}", path.file_name().and_then(|n| n.to_str()).unwrap_or("config.toml"), crabsdr_auth::random_password(8)));
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp).map_err(|e| {
        SaveError::ReadOnly(format!("{} ist für crabSDR nicht beschreibbar ({}). Rechte: Ordner und Datei für die Gruppe crabsdr schreibbar machen.", dir.display(), e))
    })?;
    let w = f.write_all(text.as_bytes()).and_then(|_| f.sync_all());
    if let Err(e) = w { let _ = std::fs::remove_file(&tmp); return Err(SaveError::Io(e.to_string())); }
    if let Ok(m) = std::fs::metadata(path) { let _ = std::fs::set_permissions(&tmp, m.permissions()); }
    if let Err(e) = std::fs::rename(&tmp, path) { let _ = std::fs::remove_file(&tmp); return Err(SaveError::Io(e.to_string())); }
    if let Ok(d) = std::fs::File::open(dir) { let _ = d.sync_all(); }
    Ok((digest(text), res))
}

pub fn list_backups(dir: &Path) -> Vec<(String, u64)> {
    let mut v: Vec<(String, u64)> = std::fs::read_dir(dir).into_iter().flatten().flatten()
        .filter_map(|e| { let n = e.file_name().to_string_lossy().to_string(); (n.starts_with("config-") && n.ends_with(".toml")).then(|| (n, e.metadata().map(|m| m.len()).unwrap_or(0))) })
        .collect();
    v.sort_by(|a, b| b.0.cmp(&a.0));
    v
}

/// Name einer Sicherung prüfen (kein Pfad!) und Pfad liefern
pub fn backup_path(dir: &Path, name: &str) -> Option<PathBuf> {
    let ok = name.starts_with("config-") && name.ends_with(".toml") && name.len() < 64 && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_'));
    ok.then(|| dir.join(name)).filter(|p| p.is_file())
}

fn prune_backups(dir: &Path) {
    for (n, _) in list_backups(dir).into_iter().skip(KEEP_BACKUPS) { let _ = std::fs::remove_file(dir.join(n)); }
}

/// JJJJMMTT-hhmmss in UTC (ohne zusätzliche Bibliothek)
pub fn stamp() -> String {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let (days, rem) = (t.div_euclid(86400), t.rem_euclid(86400));
    // Tage seit 1970 → Datum (Howard Hinnant)
    let z = days + 719468; let era = z.div_euclid(146097); let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; let y = yoe + era * 400; let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153; let d = doy - (153 * mp + 2) / 5 + 1; let m = if mp < 10 { mp + 3 } else { mp - 9 };
    format!("{:04}{:02}{:02}-{:02}{:02}{:02}", if m <= 2 { y + 1 } else { y }, m, d, rem / 3600, rem % 3600 / 60, rem % 60)
}

#[cfg(test)]
mod tests {
    #[test]
    fn verzeichnis_schalter() {
        let out = apply(BASE, &[set(&["directory", "enabled"], J::from(true))]).unwrap();
        assert!(out.contains("[directory]") && out.contains("enabled = true"));
        assert!(apply(BASE, &[set(&["directory", "server"], J::from("https://anders.example"))]).is_err(), "Server nur in der Datei");
    }

    use super::*;

    const BASE: &str = r#"# Meine Station
port = 8080
plugin_dir = "/opt/crabsdr/plugins"

[station]
name = "Alt"   # Name oben links

# erstes Band
[[sdrs]]
id = "2m"
sdr_driver = "rtl_tcp"
sdr_tcp_port = 6901
center_freq = 145000000
gain = 12.0
"#;

    fn set(path: &[&str], v: J) -> Op {
        Op::Set { path: path.iter().map(|s| s.parse::<usize>().map(Seg::Index).unwrap_or(Seg::Key(s.to_string()))).collect(), value: v }
    }

    #[test]
    fn aendern_behaelt_kommentare_und_alte_namen() {
        let out = apply(BASE, &[set(&["station", "name"], J::from("Neu")), set(&["bands", "0", "gain"], J::from(20.5)),
                                 set(&["bands", "0", "port"], J::from(6902)), set(&["ui", "chat"], J::from(false))]).unwrap();
        assert!(out.contains("# Meine Station") && out.contains("# Name oben links") && out.contains("# erstes Band"));
        assert!(out.contains("name = \"Neu\"") && out.contains("gain = 20.5"));
        assert!(out.contains("sdr_tcp_port = 6902") && !out.contains("\nport = 6902"), "alter Name im Band bleibt");
        assert!(out.contains("[ui]") && out.contains("chat = false"));
        assert!(out.contains("port = 8080"));
    }

    #[test]
    fn gesperrt_und_nicht_erlaubt() {
        assert!(apply(BASE, &[set(&["port"], J::from(80))]).is_err());
        assert!(apply(BASE, &[set(&["plugin_dir"], J::from("/tmp"))]).is_err());
        assert!(apply(BASE, &[set(&["bands", "0", "plugin_dir"], J::from("/tmp"))]).is_err());
        assert!(apply(BASE, &[set(&["station", "name"], serde_json::json!({"a": 1}))]).is_err());
        let changed = BASE.replace("plugin_dir = \"/opt/crabsdr/plugins\"", "plugin_dir = \"/tmp/boese\"");
        assert_eq!(locked_changes(BASE, &changed), vec!["plugin_dir".to_string()]);
        let extra = format!("{}\ndata_dir = \"/tmp\"\n", BASE.replace("[station]", "data_dir = \"/tmp\"\n[station]"));
        assert!(locked_changes(BASE, &extra).contains(&"data_dir".to_string()));
    }

    #[test]
    fn anhaengen_entfernen() {
        let mut t = serde_json::Map::new();
        t.insert("plugin".into(), J::from("aprs")); t.insert("freq".into(), J::from(144800000u64));
        t.insert("options".into(), serde_json::json!({"mycall": "DO1XX", "igate": "on"}));
        let out = apply(BASE, &[Op::Append { array: "decoders".into(), table: t }]).unwrap();
        assert!(out.contains("[[decoders]]") && out.contains("mycall = \"DO1XX\""));
        let out2 = apply(&out, &[set(&["decoders", "0", "options", "igate"], J::Null), set(&["decoders", "0", "enabled"], J::from(false))]).unwrap();
        assert!(!out2.contains("igate") && out2.contains("enabled = false"));
        let out3 = apply(&out2, &[Op::Remove { path: vec![Seg::Key("decoders".into()), Seg::Index(0)] }]).unwrap();
        assert!(!out3.contains("[[decoders]]"));
        let mut bad = serde_json::Map::new(); bad.insert("command".into(), J::from("rm -rf /"));
        assert!(apply(BASE, &[Op::Append { array: "decoders".into(), table: bad }]).is_err());
        let mut pf = serde_json::Map::new(); pf.insert("plugin".into(), J::from("aprs")); pf.insert("freq".into(), J::from(1u64));
        pf.insert("options".into(), serde_json::json!({"igate_passfile": "/var/lib/crabsdr/.jwt_secret"}));
        assert!(apply(BASE, &[Op::Append { array: "decoders".into(), table: pf }]).is_err(), "Pfad-Option per Formular");
        assert!(apply(&out, &[set(&["decoders", "0", "options", "log"], J::from("/etc/crabsdr/config.toml"))]).is_err());
        let mit = format!("{}\n[[decoders]]\nplugin = \"ft8\"\nfreq = 1\noptions = {{ json = \"/etc/x\" }}\n", BASE);
        assert!(!locked_changes(BASE, &mit).is_empty(), "Pfad-Option per Text");
        assert!(locked_changes(&mit, BASE).is_empty(), "wegnehmen ist erlaubt");
        let mut o = serde_json::Map::new(); o.insert("options".into(), serde_json::json!({"Böse Taste": "x"}));
        assert!(apply(BASE, &[Op::Append { array: "decoders".into(), table: o }]).is_err());
    }

    #[test]
    fn speichern_pruefen_sichern_konflikt() {
        let d = std::env::temp_dir().join(format!("crabsdr-confedit-{}", crabsdr_auth::random_password(8)));
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("config.toml");
        std::fs::write(&p, BASE).unwrap();
        let h0 = digest(BASE);
        // ungültig: falscher Typ → wird nicht gespeichert
        let kaputt = BASE.replace("gain = 12.0", "gain = \"laut\"");
        assert!(matches!(save(&p, &d.join("bak"), &h0, &kaputt), Err(SaveError::Invalid(_))));
        assert_eq!(std::fs::read_to_string(&p).unwrap(), BASE);
        // Syntaxfehler
        assert!(matches!(save(&p, &d.join("bak"), &h0, "[[bands]\n"), Err(SaveError::Invalid(_))));
        // gesperrter Schlüssel
        assert!(matches!(save(&p, &d.join("bak"), &h0, &BASE.replace("port = 8080", "port = 80")), Err(SaveError::Invalid(_))));
        // gültig
        let neu = apply(BASE, &[set(&["station", "name"], J::from("Neu"))]).unwrap();
        let (h1, _) = save(&p, &d.join("bak"), &h0, &neu).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), neu);
        assert_eq!(list_backups(&d.join("bak")).len(), 1);
        // Konflikt: mit alter Prüfsumme
        assert!(matches!(save(&p, &d.join("bak"), &h0, BASE), Err(SaveError::Conflict)));
        assert_eq!(h1, digest(&neu));
        // Sicherungsnamen ohne Pfad
        assert!(backup_path(&d.join("bak"), "../config.toml").is_none());
        assert!(backup_path(&d.join("bak"), &list_backups(&d.join("bak"))[0].0).is_some());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn zeitstempel() {
        let s = stamp();
        assert_eq!(s.len(), 15);
        assert!(s.starts_with("20"));
    }
}
