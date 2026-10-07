use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

fn default_true() -> bool { true }
fn default_chat_keep_hours() -> u32 { 3 }
fn default_chat_keep_lines() -> u32 { 50 }
fn default_max_listeners() -> u32 { 50 }
fn default_max_per_ip() -> u32 { 10 }
fn default_max_channels() -> u32 { 16 }
fn default_max_streams() -> u32 { 6 }
fn default_iq_stream() -> String { "admin".into() }
fn default_max_iq() -> u32 { 1 }
fn default_dv_max() -> u32 { 3 }

/// Ein Band (`[[bands]]`, früher `[[sdrs]]`) = ein Empfänger. Neue Schlüsselnamen (driver, device, host, port, mode),
/// die alten (sdr_driver, sdr_device, sdr_tcp_host, sdr_tcp_port, default_mode) gelten weiter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SdrInstanceConfig {
    pub id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default = "default_sdr_driver", rename = "driver", alias = "sdr_driver")]
    pub sdr_driver: String,
    #[serde(default = "default_sdr_device", rename = "device", alias = "sdr_device")]
    pub sdr_device: String,
    #[serde(default = "default_tcp_host", rename = "host", alias = "sdr_tcp_host")]
    pub sdr_tcp_host: String,
    #[serde(default = "default_tcp_port", rename = "port", alias = "sdr_tcp_port")]
    pub sdr_tcp_port: u16,
    #[serde(default = "default_center_freq")]
    pub center_freq: u64,
    #[serde(default = "default_sample_rate")]
    pub sample_rate: u32,
    #[serde(default = "default_gain")]
    pub gain: f64,
    #[serde(default)]
    pub ppm: i32,
    /// Frequenzkorrektur in Software (ppm, mit Nachkommastellen, jeder Treiber): Anzeige und Abstimmung werden um diesen
    /// Faktor verschoben. Positiv, wenn bekannte Signale zu tief angezeigt werden. Für Empfänger ohne TCXO (z. B. RSP1),
    /// deren Hardware-`ppm` nur ganze Werte kennt oder nicht greift.
    #[serde(default)]
    pub freq_correction_ppm: f64,
    /// Punkte der FFT; 0 (Voreinstellung) = automatisch, so dass ein Bin etwa 500 Hz breit ist (2,048 MS/s → 4096,
    /// 8 MS/s → 16384). Mit gröberen Bins pfeift der Kanalfilter im Blocktakt (MSi2500 bei 8 MS/s, 29.09.).
    #[serde(default)]
    pub fft_size: usize,
    #[serde(default = "default_fft_fps")]
    pub fft_fps: u32,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub admin_only: bool,
    /// ohne Anmeldung hörbar (Voreinstellung: ja; nur bei eingerichteter Benutzerverwaltung von Bedeutung)
    #[serde(default = "default_true")]
    pub guest: bool,
    #[serde(default)]
    pub bias_tee: bool,
    /// Betriebsart beim Start (fm, am, usb, lsb, cw)
    #[serde(default, rename = "mode", alias = "default_mode")]
    pub default_mode: Option<String>,
    /// Per-element gain values (e.g. {"IFGR": 40, "RFGR": 2})
    #[serde(default)]
    pub gain_elements: Option<HashMap<String, f64>>,
    /// Kurzbeschreibung des Bandes für die Oberfläche (Reiter-Tooltip), z. B. "FM-Relais, APRS, FT8"
    #[serde(default)]
    pub note: Option<String>,
    /// S-Meter-Kalibrierung in dB (dBm = dBFS − gain + smeter_cal), bezogen auf 0 dB Verstärkung
    #[serde(default)]
    pub smeter_cal: Option<f64>,
    /// Nur driver = "rx_sdr": Rohformat der Samples – "cs16" (Voreinstellung, volle Dynamik), "cf32" (Module, die nur
    /// Gleitkomma liefern), "cu8" (wie RTL-Sticks, halbe Datenmenge)
    #[serde(default)]
    pub format: Option<String>,
    /// Nur driver = "rx_sdr": Geräte-Einstellungen für SoapySDR (rx_sdr -t), z. B. "transfer=BULK" (SoapyMiri: USB-Bulk
    /// statt isochron) oder "biastee=true"
    #[serde(default)]
    pub settings: Option<String>,
}

/// Stationsangaben (Name, Untertitel, Adresse, Locator) für die Oberfläche.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StationConfig {
    #[serde(default = "default_station_name")]
    pub name: String,
    #[serde(default)]
    pub subtitle: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub locator: String,
    /// Standort (Dezimalgrad) für Entfernungen im Logbuch und auf der Digital-Seite; ohne Angabe aus dem Locator
    #[serde(default)]
    pub lat: Option<f64>,
    #[serde(default)]
    pub lon: Option<f64>,
    /// Betreiber (Impressum auf der Info-Seite): Name/Rufzeichen, Anschrift (Zeilen mit \n), Kontakt (E-Mail)
    #[serde(default)]
    pub operator: String,
    #[serde(default)]
    pub address: String,
    #[serde(default)]
    pub contact: String,
}
fn default_station_name() -> String { "crabSDR".into() }
impl Default for StationConfig {
    fn default() -> Self { Self { name: default_station_name(), subtitle: String::new(), url: String::new(), locator: String::new(), lat: None, lon: None, operator: String::new(), address: String::new(), contact: String::new() } }
}
impl StationConfig {
    /// Standort: lat/lon, sonst Mitte des Locators (4/6/8 Zeichen), sonst None
    pub fn home(&self) -> Option<(f64, f64)> {
        if let (Some(a), Some(b)) = (self.lat, self.lon) { return Some((a, b)); }
        locator_to_latlon(&self.locator)
    }
}

/// Maidenhead-Locator (4, 6 oder 8 Zeichen) → Mitte des Feldes (lat, lon)
pub fn locator_to_latlon(loc: &str) -> Option<(f64, f64)> {
    let c: Vec<char> = loc.trim().to_uppercase().chars().collect();
    if c.len() < 4 || !('A'..='R').contains(&c[0]) || !('A'..='R').contains(&c[1]) || !c[2].is_ascii_digit() || !c[3].is_ascii_digit() { return None; }
    let mut lon = (c[0] as u8 - b'A') as f64 * 20.0 - 180.0 + (c[2] as u8 - b'0') as f64 * 2.0;
    let mut lat = (c[1] as u8 - b'A') as f64 * 10.0 - 90.0 + (c[3] as u8 - b'0') as f64;
    if c.len() >= 6 && ('A'..='X').contains(&c[4]) && ('A'..='X').contains(&c[5]) {
        lon += (c[4] as u8 - b'A') as f64 * 5.0 / 60.0; lat += (c[5] as u8 - b'A') as f64 * 2.5 / 60.0;
        if c.len() >= 8 && c[6].is_ascii_digit() && c[7].is_ascii_digit() {
            lon += (c[6] as u8 - b'0') as f64 * 0.5 / 60.0 + 0.25 / 60.0; lat += (c[7] as u8 - b'0') as f64 * 0.25 / 60.0 + 0.125 / 60.0;
        } else { lon += 2.5 / 60.0; lat += 1.25 / 60.0; }
    } else { lon += 1.0; lat += 0.5; }
    Some((lat, lon))
}

/// Ein Decoder-Einsatz (`[[decoders]]`): welches Plugin auf welcher Frequenz lauscht. Das Plugin (Verzeichnis
/// `plugin_dir/<plugin>/decoder.json`) beschreibt, was es braucht (Betriebsart, Abtastrate, Bandbreite, Programm);
/// die Konfiguration nur, wo es hören soll. Mehrere Einsätze desselben Plugins sind erlaubt (z. B. SSTV auf zwei Frequenzen).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecoderInstanceConfig {
    pub plugin: String,
    /// Frequenz in Hz
    pub freq: u64,
    /// Kennung (Standard: `<plugin>-<kHz>`), Datenverzeichnis `data_dir/decoders/<id>`
    #[serde(default)]
    pub id: Option<String>,
    /// Band-ID; ohne Angabe das Band, in dem die Frequenz liegt
    #[serde(default)]
    pub band: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Treffer für alle Hörer sichtbar (false: nur in der Server-Statusliste, z. B. für Versuche)
    #[serde(default = "default_true")]
    pub public: bool,
    /// Freie Optionen für das Plugin (als `CRAB_OPT_<NAME>` und `{opt.name}` im Befehl)
    #[serde(default)]
    pub options: HashMap<String, String>,
}

/// Decoder-Treffer an einen MQTT-Broker veröffentlichen (`[mqtt]`): Thema `<topic>/<decoder-id>/<art>`, JSON wie
/// `/api/decoders/events` plus `station`; `<topic>/status` = online/offline (Last Will, gehalten).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MqttConfig {
    pub host: String,
    #[serde(default = "default_mqtt_port")]
    pub port: u16,
    #[serde(default)]
    pub username: Option<String>,
    /// Datei mit dem Passwort (nur auf dem Gerät, nicht in der Konfiguration)
    #[serde(default)]
    pub password_file: Option<PathBuf>,
    /// Themenpräfix, z. B. "crabsdr/meinestation"
    pub topic: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}
fn default_mqtt_port() -> u16 { 1883 }

/// Eintrag im öffentlichen crabSDR-Verzeichnis (`[directory]`, wie das Receiverbook bei OpenWebRX). Aus, bis der Sysop es
/// einschaltet. Dann meldet die Station alle 5 min Name, Standort, öffentliche Bänder und Decoder und die Hörerzahl an
/// `server`; das Verzeichnis prüft die Angaben, indem es `station.url` + `/api/directory` selbst abruft.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectoryConfig {
    #[serde(default)]
    pub enabled: bool,
    /// Verzeichnis-Server (Voreinstellung https://crabsdr.de)
    #[serde(default = "default_directory_server")]
    pub server: String,
}
fn default_directory_server() -> String { "https://crabsdr.de".into() }
impl Default for DirectoryConfig {
    fn default() -> Self { Self { enabled: false, server: default_directory_server() } }
}

/// Oberfläche (`[ui]`). Alles ist optional: ohne Angabe gilt die Voreinstellung (meist „an“ bzw. „automatisch“).
/// digital/logbook/info erscheinen automatisch, wenn die Seite (digi/, logbuch/, info/) im Stationsordner liegt.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UiConfig {
    #[serde(default)] pub chat: Option<bool>,
    #[serde(default)] pub status: Option<bool>,
    #[serde(default)] pub decoders: Option<bool>,
    #[serde(default)] pub recording: Option<bool>,
    #[serde(default)] pub digital: Option<bool>,
    #[serde(default)] pub logbook: Option<bool>,
    #[serde(default)] pub info: Option<bool>,
    /// Knopf „Anmelden“ auf der Hören-Seite (Voreinstellung: an, wenn es Mitglieder- oder Admin-Bänder gibt)
    #[serde(default)] pub login: Option<bool>,
    /// Hinweis oben auf der Seite, z. B. "Wartung heute ab 20 Uhr"
    #[serde(default)] pub banner: Option<String>,
    /// Links unten auf der Seite
    #[serde(default)] pub impressum: Option<String>,
    #[serde(default)] pub datenschutz: Option<String>,
    /// Admin-Link (nur sichtbar, wenn die Seite über eine private Adresse aufgerufen wird)
    #[serde(default)] pub admin: Option<String>,
}

/// Top-level server configuration (multi-SDR).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_port")]
    pub port: u16,

    // Pfade: leer = automatisch (siehe `resolve_paths`)
    #[serde(default, skip_serializing_if = "path_is_empty")]
    pub plugin_dir: PathBuf,
    #[serde(default, skip_serializing_if = "path_is_empty")]
    pub frontend_dir: PathBuf,
    #[serde(default, skip_serializing_if = "path_is_empty")]
    pub data_dir: PathBuf,
    /// Stationsordner: hat Vorrang vor frontend_dir (Logo, Schnellwahl, Relaisliste, Zusatzseiten digi/, logbuch/, info/ …;
    /// wer will, kann hier jede Datei der Oberfläche ersetzen)
    #[serde(default)]
    pub site_dir: Option<PathBuf>,
    /// Oberfläche
    #[serde(default)]
    pub ui: UiConfig,

    // Auth
    #[serde(default = "default_jwt_secret_env")]
    pub jwt_secret_env: String,
    #[serde(default, skip_serializing_if = "path_is_empty")]
    pub db_path: PathBuf,

    /// Bänder (`[[bands]]`, früher `[[sdrs]]`)
    #[serde(default, rename = "bands", alias = "sdrs")]
    pub sdrs: Vec<SdrInstanceConfig>,

    /// Decoder-Einsätze
    #[serde(default)]
    pub decoders: Vec<DecoderInstanceConfig>,
    /// Treffer zusätzlich per MQTT veröffentlichen
    #[serde(default)]
    pub mqtt: Option<MqttConfig>,
    /// Im öffentlichen Verzeichnis (crabsdr.de) gelistet werden
    #[serde(default)]
    pub directory: DirectoryConfig,



    /// Opus-Bitrate je Kanal in bit/s (24 kHz mono; 32 kbit/s reicht für NFM-Sprache, 48 für Rundfunk)
    #[serde(default = "default_opus_bitrate")]
    pub opus_bitrate: u32,
    /// Chat und Logbuch der Hörer (SQLite in data_dir); false nur, wenn ein anderer Dienst sie übernimmt
    #[serde(default = "default_true")]
    pub builtin_chat: bool,
    /// Chat behält nur die letzten so vielen Zeilen, Älteres fällt hinten raus (0 = alle behalten); das Logbuch bleibt
    #[serde(default = "default_chat_keep_lines")]
    pub chat_keep_lines: u32,
    /// Zusätzlich Chat-Zeilen nach so vielen Stunden löschen (0 = aus)
    #[serde(default = "default_chat_keep_hours")]
    pub chat_keep_hours: u32,
    /// Verbund-Chat: Chatzeilen über crabsdr.de mit allen teilnehmenden Stationen teilen (braucht `[directory] enabled`).
    /// Die Station reicht weiter, Hörer-IPs verlassen sie nicht. Voreinstellung aus.
    #[serde(default)]
    pub chat_verbund: bool,
    /// Lastgrenzen je Band, 0 = keine Grenze. Verbindungen insgesamt und je Absenderadresse; verschiedene Kanäle (jede
    /// andere Frequenz, Betriebsart oder Bandbreite kostet einen DSP-Kanal, Hörer auf demselben Kanal teilen ihn);
    /// Streams über /stream/… (zählen zusätzlich bei den Verbindungen mit)
    #[serde(default = "default_max_listeners")]
    pub max_listeners: u32,
    #[serde(default = "default_max_per_ip")]
    pub max_per_ip: u32,
    #[serde(default = "default_max_channels")]
    pub max_channels: u32,
    #[serde(default = "default_max_streams")]
    pub max_streams: u32,
    /// I/Q-Stream (`/stream/<kHz>/iq.wav`, komplexes Basisband für externe Decoder, ab 0,5 Mbit/s je Hörer):
    /// `off`, `admin` (nur Sysop), `users` (angemeldete Hörer), `all` (auch Gäste); dazu die Zahl gleichzeitiger I/Q-Streams
    #[serde(default = "default_iq_stream")]
    pub iq_stream: String,
    #[serde(default = "default_max_iq")]
    pub max_iq: u32,
    /// Fester Schlüssel für Streams (`?token=<stream_key>`): gilt wie eine Sysop-Anmeldung, aber nur für /stream/…, läuft nicht
    /// ab, wird auf der Admin-Seite gesetzt. Leer = aus. Lang und zufällig wählen; bei Verlust einfach ändern.
    #[serde(default)]
    pub stream_key: String,
    /// Betriebsart DV: Hörer können FreeDV (Plugin `freedv`) auf ihrer Frequenz dekodieren lassen; so viele solcher
    /// Decoder laufen höchstens gleichzeitig (je einer pro Frequenz, Hörer teilen ihn; endet nach 90 s ohne Hörer). 0 = aus
    #[serde(default = "default_dv_max")]
    pub dv_max: u32,
    /// Stationsangaben für die neutrale Oberfläche (Platzhalter in index.html)
    #[serde(default)]
    pub station: StationConfig,
    /// Veraltet: S-Meter-Kalibrierung je Band als eigene Tabelle; neu `smeter_cal` im Band. Wird beim Laden ins Band übernommen.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub smeter_cal: HashMap<String, f64>,
    /// Opus-Komplexität 0–10 (CPU gegen Qualität; 3 ist für NFM-Sprache ausreichend, gemessen: 25 Kanäle 21,9 % statt 29,9 % bei 5)
    #[serde(default = "default_opus_complexity")]
    pub opus_complexity: u32,
    /// Datei, aus der die Konfiguration gelesen wurde
    #[serde(skip)]
    pub source: Option<PathBuf>,
}

fn path_is_empty(p: &std::path::Path) -> bool { p.as_os_str().is_empty() }

fn default_opus_bitrate() -> u32 { 32_000 }
fn default_opus_complexity() -> u32 { 3 }
fn default_jwt_secret_env() -> String { "CRABSDR_JWT_SECRET".into() }

/// Legacy flat config (backward compat: single SDR without [[sdrs]] array).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_port")]
    pub port: u16,

    // SDR settings
    #[serde(default = "default_sdr_driver")]
    pub sdr_driver: String,
    #[serde(default = "default_sdr_device")]
    pub sdr_device: String,
    #[serde(default = "default_tcp_host")]
    pub sdr_tcp_host: String,
    #[serde(default = "default_tcp_port")]
    pub sdr_tcp_port: u16,
    #[serde(default = "default_center_freq")]
    pub center_freq: u64,
    #[serde(default = "default_sample_rate")]
    pub sample_rate: u32,
    #[serde(default = "default_gain")]
    pub gain: f64,
    #[serde(default)]
    pub ppm: i32,

    // DSP settings
    #[serde(default = "default_fft_size")]
    pub fft_size: usize,
    #[serde(default = "default_fft_fps")]
    pub fft_fps: u32,
    #[serde(default = "default_audio_rate")]
    pub audio_rate: u32,

    // Paths
    #[serde(default = "default_plugin_dir")]
    pub plugin_dir: PathBuf,
    #[serde(default = "default_frontend_dir")]
    pub frontend_dir: PathBuf,
    #[serde(default = "default_data_dir")]
    pub data_dir: PathBuf,
}

fn default_port() -> u16 { 8080 }
fn default_sdr_driver() -> String { "rtl_tcp".into() }
fn default_sdr_device() -> String { "0".into() }
fn default_tcp_host() -> String { "127.0.0.1".into() }
fn default_tcp_port() -> u16 { 1234 }
fn default_center_freq() -> u64 { 145_500_000 }
fn default_sample_rate() -> u32 { 2_048_000 }
fn default_gain() -> f64 { 40.0 }
fn default_fft_size() -> usize { 4096 }

/// FFT-Größe für ~500-Hz-Bins (mindestens 4096): 2,048 MS/s → 4096, 3 MS/s → 8192, 8 MS/s → 16384
pub fn auto_fft_size(sample_rate: u32) -> usize {
    ((sample_rate as usize).div_ceil(500)).next_power_of_two().clamp(4096, 65536)
}
fn default_fft_fps() -> u32 { 50 }
fn default_audio_rate() -> u32 { 48000 }
fn default_plugin_dir() -> PathBuf { PathBuf::from("plugins") }
fn default_frontend_dir() -> PathBuf { PathBuf::from("web") }
fn default_data_dir() -> PathBuf { PathBuf::from("/data") }

impl Default for Config {
    fn default() -> Self {
        Self {
            port: default_port(),
            sdr_driver: default_sdr_driver(),
            sdr_device: default_sdr_device(),
            sdr_tcp_host: default_tcp_host(),
            sdr_tcp_port: default_tcp_port(),
            center_freq: default_center_freq(),
            sample_rate: default_sample_rate(),
            gain: default_gain(),
            ppm: 0,
            fft_size: default_fft_size(),
            fft_fps: default_fft_fps(),
            audio_rate: default_audio_rate(),
            plugin_dir: default_plugin_dir(),
            frontend_dir: default_frontend_dir(),
            data_dir: default_data_dir(),
        }
    }
}

impl Config {
    /// Load config: TOML file → env var overrides
    pub fn load() -> Self {
        let mut config = Self::load_from_file();
        config.apply_env_overrides();
        config
    }

    fn load_from_file() -> Self {
        // Check CRABSDR_CONFIG env, then default paths
        let paths = [
            std::env::var("CRABSDR_CONFIG").ok().map(PathBuf::from),
            Some(PathBuf::from("/data/config.toml")),
            Some(PathBuf::from("config.toml")),
        ];

        for path in paths.iter().flatten() {
            if path.exists() {
                match std::fs::read_to_string(path) {
                    Ok(content) => match toml::from_str(&content) {
                        Ok(config) => {
                            info!("Loaded config from {}", path.display());
                            return config;
                        }
                        Err(e) => warn!("Failed to parse {}: {}", path.display(), e),
                    },
                    Err(e) => warn!("Failed to read {}: {}", path.display(), e),
                }
            }
        }

        Self::default()
    }

    fn apply_env_overrides(&mut self) {
        if let Ok(v) = std::env::var("PORT") { if let Ok(v) = v.parse() { self.port = v; } }
        if let Ok(v) = std::env::var("SDR_DRIVER") { self.sdr_driver = v; }
        if let Ok(v) = std::env::var("SDR_DEVICE") { self.sdr_device = v; }
        if let Ok(v) = std::env::var("SDR_TCP_HOST") { self.sdr_tcp_host = v; }
        if let Ok(v) = std::env::var("SDR_TCP_PORT") { if let Ok(v) = v.parse() { self.sdr_tcp_port = v; } }
        if let Ok(v) = std::env::var("CENTER_FREQ") { if let Ok(v) = v.parse() { self.center_freq = v; } }
        if let Ok(v) = std::env::var("SAMPLE_RATE") { if let Ok(v) = v.parse() { self.sample_rate = v; } }
        if let Ok(v) = std::env::var("GAIN") { if let Ok(v) = v.parse() { self.gain = v; } }
        if let Ok(v) = std::env::var("PPM") { if let Ok(v) = v.parse() { self.ppm = v; } }
        if let Ok(v) = std::env::var("FFT_SIZE") { if let Ok(v) = v.parse() { self.fft_size = v; } }
        if let Ok(v) = std::env::var("FFT_FPS") { if let Ok(v) = v.parse() { self.fft_fps = v; } }
        if let Ok(v) = std::env::var("AUDIO_RATE") { if let Ok(v) = v.parse() { self.audio_rate = v; } }
        if let Ok(v) = std::env::var("PLUGIN_DIR") { self.plugin_dir = PathBuf::from(v); }
        if let Ok(v) = std::env::var("FRONTEND_DIR") { self.frontend_dir = PathBuf::from(v); }
    }

    pub fn save(&self, path: &Path) -> Result<(), crate::CrabSdrError> {
        let content = toml::to_string_pretty(self)
            .map_err(|e| crate::CrabSdrError::Config(e.to_string()))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, content)?;
        Ok(())
    }

    pub fn config_path(&self) -> PathBuf {
        self.data_dir.join("config.toml")
    }

    /// Convert legacy flat Config into a single-SDR ServerConfig.
    pub fn to_server_config(&self) -> ServerConfig {
        let sdr = SdrInstanceConfig {
            id: "default".into(),
            label: format!("{:.3} MHz", self.center_freq as f64 / 1e6),
            sdr_driver: self.sdr_driver.clone(),
            sdr_device: self.sdr_device.clone(),
            sdr_tcp_host: self.sdr_tcp_host.clone(),
            sdr_tcp_port: self.sdr_tcp_port,
            center_freq: self.center_freq,
            sample_rate: self.sample_rate,
            gain: self.gain,
            ppm: self.ppm,
            freq_correction_ppm: 0.0,
            fft_size: self.fft_size,
            fft_fps: self.fft_fps,
            enabled: true,
            admin_only: false,
            guest: false,
            bias_tee: false,
            default_mode: None,
            gain_elements: None,
            note: None,
            smeter_cal: None,
            format: None,
            settings: None,
        };
        ServerConfig {
            port: self.port,
            plugin_dir: self.plugin_dir.clone(),
            frontend_dir: self.frontend_dir.clone(),
            data_dir: self.data_dir.clone(),
            site_dir: None,
            ui: UiConfig::default(),
            jwt_secret_env: default_jwt_secret_env(),
            db_path: self.data_dir.join("crabsdr.db"),
            sdrs: vec![sdr],
            decoders: vec![],
            mqtt: None,
            directory: DirectoryConfig::default(),
            opus_bitrate: default_opus_bitrate(),
            opus_complexity: default_opus_complexity(),
            smeter_cal: HashMap::new(),
            builtin_chat: true,
            chat_keep_lines: 50,
            chat_keep_hours: 3,
            chat_verbund: false,
            max_listeners: 50,
            max_per_ip: 10,
            max_channels: 16,
            max_streams: 6,
            iq_stream: "admin".into(),
            max_iq: 1,
            stream_key: String::new(),
            dv_max: 3,
            station: StationConfig::default(),
            source: None,
        }
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Config::default().to_server_config()
    }
}

/// Ergebnis des Ladens: Konfiguration + Hinweise (unbekannte Schlüssel, alte Namen, fehlende Datei …)
#[derive(Debug)]
pub struct Loaded {
    pub config: ServerConfig,
    pub warnings: Vec<String>,
}

/// Wo die Konfiguration gesucht wird (erste vorhandene Datei gilt)
pub fn config_candidates(explicit: Option<&Path>) -> Vec<PathBuf> {
    if let Some(p) = explicit { return vec![p.to_path_buf()]; }
    let mut v = Vec::new();
    if let Ok(p) = std::env::var("CRABSDR_CONFIG") { if !p.is_empty() { return vec![PathBuf::from(p)]; } }
    v.push(PathBuf::from("/etc/crabsdr/config.toml"));
    v.push(PathBuf::from("/data/config.toml"));
    v.push(PathBuf::from("config.toml"));
    v
}

/// Alte Schlüsselnamen → neue (nur Hinweis, beide gelten)
const RENAMED: &[(&str, &str)] = &[("sdrs", "bands"), ("sdr_driver", "driver"), ("sdr_device", "device"), ("sdr_tcp_host", "host"), ("sdr_tcp_port", "port"), ("default_mode", "mode")];

impl ServerConfig {
    /// Konfiguration laden (explizite Datei, sonst $CRABSDR_CONFIG, /etc/crabsdr/config.toml, /data/config.toml, ./config.toml).
    /// Ist eine Datei da, aber nicht lesbar oder fehlerhaft, ist das ein Fehler – nie still mit Voreinstellungen weiter.
    pub fn load_checked(explicit: Option<&Path>) -> Result<Loaded, String> {
        let mut warnings = Vec::new();
        let cands = config_candidates(explicit);
        let found = cands.iter().find(|p| p.exists()).cloned();
        let Some(path) = found else {
            if explicit.is_some() || std::env::var("CRABSDR_CONFIG").is_ok_and(|v| !v.is_empty()) {
                return Err(format!("Konfiguration {} nicht gefunden", cands[0].display()));
            }
            warnings.push("keine Konfiguration gefunden (/etc/crabsdr/config.toml, /data/config.toml, ./config.toml) – Voreinstellungen, ein Band per Umgebungsvariablen".into());
            let mut legacy = Config::default();
            legacy.apply_env_overrides();
            let mut sc = legacy.to_server_config();
            sc.data_dir = PathBuf::new(); sc.frontend_dir = PathBuf::new(); sc.plugin_dir = PathBuf::new(); sc.db_path = PathBuf::new();
            sc.apply_env_overrides();
            sc.finish(&mut warnings);
            return Ok(Loaded { config: sc, warnings });
        };
        let content = std::fs::read_to_string(&path).map_err(|e| format!("{}: nicht lesbar: {} (Rechte? Der Dienst läuft als Benutzer crabsdr)", path.display(), e))?;
        Self::from_content(&content, &path)
    }

    /// Konfiguration aus Text (so, als stünde er in `path`): gleiche Prüfungen wie beim Laden der Datei
    pub fn from_content(content: &str, path: &Path) -> Result<Loaded, String> {
        let mut warnings = Vec::new();
        let path = path.to_path_buf();
        let value: toml::Value = toml::from_str(content).map_err(|e| format!("{}: {}", path.display(), e))?;
        // Unbekannte Schlüssel sammeln (Tippfehler fallen sonst nie auf)
        let mut unknown = Vec::new();
        let de = toml::Deserializer::new(content);
        let mut sc: ServerConfig = serde_ignored::deserialize(de, |p| unknown.push(p.to_string())).map_err(|e| format!("{}: {}", path.display(), e))?;
        for u in unknown {
            warnings.push(format!("unbekannter Schlüssel „{}“ – wird ignoriert (Tippfehler?)", u));
        }
        // Alte Namen melden
        let mut old = Vec::new();
        if let Some(t) = value.as_table() {
            if t.contains_key("sdrs") { old.push("sdrs → bands"); }
            let bands = t.get("bands").or_else(|| t.get("sdrs")).and_then(|b| b.as_array()).cloned().unwrap_or_default();
            for (o, n) in RENAMED.iter().skip(1) {
                if bands.iter().any(|b| b.get(*o).is_some()) { old.push(Box::leak(format!("{} → {}", o, n).into_boxed_str())); }
            }
            if t.contains_key("smeter_cal") { old.push("[smeter_cal] → smeter_cal im Band"); }
            if sc.sdrs.is_empty() && (t.contains_key("sdr_driver") || t.contains_key("center_freq")) {
                // ganz altes Format: ein Band direkt oben in der Datei
                let legacy: Config = toml::from_str(content).map_err(|e| format!("{}: {}", path.display(), e))?;
                let mut l = legacy.to_server_config();
                l.station = sc.station.clone(); l.ui = sc.ui.clone(); l.decoders = sc.decoders.clone(); l.mqtt = sc.mqtt.clone();
                sc = l;
                warnings.push("altes Format (ein Band oben in der Datei) – besser als [[bands]] schreiben".into());
            }
        }
        if !old.is_empty() { warnings.push(format!("alte Schlüsselnamen (gelten weiter): {}", old.join(", "))); }
        if sc.sdrs.is_empty() { return Err(format!("{}: kein Band – mindestens ein [[bands]]-Eintrag nötig", path.display())); }
        sc.source = Some(path);
        sc.apply_env_overrides();
        sc.finish(&mut warnings);
        Ok(Loaded { config: sc, warnings })
    }

    /// Alter Einstieg: wie `load_checked`, bricht bei Fehlern ab
    pub fn load() -> Self {
        match Self::load_checked(None) {
            Ok(l) => { for w in &l.warnings { warn!("Konfiguration: {}", w); } l.config }
            Err(e) => { eprintln!("crabSDR: {}", e); std::process::exit(2); }
        }
    }

    /// Nach dem Lesen: Pfade ergänzen, FFT-Größe automatisch, veraltete [smeter_cal]-Tabelle in die Bänder übernehmen
    fn finish(&mut self, warnings: &mut Vec<String>) {
        self.resolve_paths();
        for b in self.sdrs.iter_mut() {
            if b.fft_size == 0 { b.fft_size = auto_fft_size(b.sample_rate); }
            else if !b.fft_size.is_power_of_two() || b.fft_size < 256 {
                warnings.push(format!("Band „{}“: fft_size {} ist keine Zweierpotenz ≥ 256 – automatisch", b.id, b.fft_size));
                b.fft_size = auto_fft_size(b.sample_rate);
            }
        }
        for (band, v) in std::mem::take(&mut self.smeter_cal) {
            match self.sdrs.iter_mut().find(|b| b.id == band) {
                Some(b) => { if b.smeter_cal.is_none() { b.smeter_cal = Some(v); } }
                None => warnings.push(format!("[smeter_cal] „{}“: kein Band mit dieser id", band)),
            }
        }
    }

    /// Leere Pfade automatisch setzen: Oberfläche und Plugins neben dem Programm (…/bin/crabsdr-server → …/web, …/plugins),
    /// sonst /opt/crabsdr/…, sonst ./web bzw. ./plugins; Daten /data (Docker), /var/lib/crabsdr oder ./data; Datenbank im Datenordner.
    pub fn resolve_paths(&mut self) {
        let base = std::env::current_exe().ok().and_then(|e| e.parent().and_then(|b| b.parent()).map(|p| p.to_path_buf()));
        let pick = |name: &str, marker: &str| -> PathBuf {
            let mut c: Vec<PathBuf> = Vec::new();
            if let Some(b) = &base { c.push(b.join(name)); }
            c.push(PathBuf::from("/opt/crabsdr").join(name));
            c.push(PathBuf::from(name));
            c.push(PathBuf::from("..").join(name));
            c.iter().find(|p| p.join(marker).exists() || (marker.is_empty() && p.is_dir())).cloned().unwrap_or_else(|| PathBuf::from(name))
        };
        if self.frontend_dir.as_os_str().is_empty() { self.frontend_dir = pick("web", "index.html"); }
        if self.plugin_dir.as_os_str().is_empty() { self.plugin_dir = pick("plugins", ""); }
        if self.data_dir.as_os_str().is_empty() {
            let docker = self.source.as_deref() == Some(Path::new("/data/config.toml"));
            self.data_dir = if docker { PathBuf::from("/data") }
                else if Path::new("/var/lib/crabsdr").is_dir() { PathBuf::from("/var/lib/crabsdr") }
                else if self.source.is_none() && Path::new("/data").is_dir() { PathBuf::from("/data") }
                else { PathBuf::from("data") };
        }
        if self.db_path.as_os_str().is_empty() { self.db_path = self.data_dir.join("crabsdr.db"); }
    }

    /// Plausibilität (ohne Dateisystem/Programme; das prüft `crabsdr-server --check` zusätzlich): (Fehler, Hinweise)
    pub fn validate(&self) -> (Vec<String>, Vec<String>) {
        let (mut err, mut warn) = (Vec::new(), Vec::new());
        let mut ids = std::collections::HashSet::new();
        for b in &self.sdrs {
            if b.id.is_empty() || b.id.contains('/') || b.id.contains(' ') { err.push(format!("Band „{}“: id darf nicht leer sein und keine Leerzeichen oder / enthalten", b.id)); }
            if !ids.insert(b.id.clone()) { err.push(format!("Band „{}“ doppelt", b.id)); }
            if b.center_freq == 0 { err.push(format!("Band „{}“: center_freq fehlt", b.id)); }
            if !["rtl_sdr", "rtl_tcp", "hackrf", "rx_sdr", "soapy", "airspy", "airspyhf", "file", "iq_file"].contains(&b.sdr_driver.as_str()) {
                warn.push(format!("Band „{}“: driver „{}“ unbekannt (rtl_sdr, rtl_tcp, hackrf, rx_sdr)", b.id, b.sdr_driver));
            }
            if let Some(st) = &b.settings {
                if !st.chars().all(|c| c.is_ascii_alphanumeric() || "=,_.-".contains(c)) || !st.split(',').all(|kv| kv.split_once('=').is_some_and(|(k, v)| !k.is_empty() && !v.is_empty())) {
                    err.push(format!("Band „{}“: settings „{}“ – Form key=wert,key2=wert2 (Buchstaben, Ziffern, _ . -)", b.id, st));
                } else if !["rx_sdr", "soapy"].contains(&b.sdr_driver.as_str()) { warn.push(format!("Band „{}“: settings gilt nur für driver = \"rx_sdr\" – wird ignoriert", b.id)); }
            }
            if let Some(f) = &b.format {
                if !["cs16", "cf32", "cu8"].contains(&f.to_ascii_lowercase().as_str()) { err.push(format!("Band „{}“: format „{}“ unbekannt (cs16, cf32, cu8)", b.id, f)); }
                else if !["rx_sdr", "soapy"].contains(&b.sdr_driver.as_str()) { warn.push(format!("Band „{}“: format gilt nur für driver = \"rx_sdr\" – wird ignoriert", b.id)); }
            }
            if let Some(m) = &b.default_mode { if !["fm", "am", "usb", "lsb", "cw", "nfm", "wfm"].contains(&m.to_lowercase().as_str()) { warn.push(format!("Band „{}“: mode „{}“ unbekannt", b.id, m)); } }
        }
        let mut dids = std::collections::HashSet::new();
        for d in &self.decoders {
            let id = d.id.clone().unwrap_or_else(|| format!("{}-{}", d.plugin, d.freq / 1000));
            if !dids.insert(id.clone()) { err.push(format!("Decoder „{}“ doppelt – id = \"…\" setzen", id)); }
            let inside = |b: &SdrInstanceConfig| { let h = b.sample_rate as u64 / 2; d.freq + h > b.center_freq && d.freq < b.center_freq + h };
            match &d.band {
                Some(bid) => match self.sdrs.iter().find(|b| &b.id == bid) {
                    None => err.push(format!("Decoder „{}“: band „{}“ gibt es nicht", id, bid)),
                    Some(b) if !inside(b) => err.push(format!("Decoder „{}“: {:.4} MHz liegt nicht im Band „{}“", id, d.freq as f64 / 1e6, bid)),
                    _ => {}
                },
                None => if d.enabled && !self.sdrs.iter().any(|b| b.enabled && inside(b)) { err.push(format!("Decoder „{}“: {:.4} MHz liegt in keinem Band", id, d.freq as f64 / 1e6)); },
            }
        }
        if let Some(m) = &self.mqtt {
            if m.enabled {
                if m.host.is_empty() || m.topic.is_empty() { err.push("[mqtt]: host und topic nötig".into()); }
                if let Some(f) = &m.password_file { if std::fs::metadata(f).is_err() { warn.push(format!("[mqtt]: password_file {} nicht lesbar", f.display())); } }
            }
        }
        if self.directory.enabled {
            let u = self.station.url.trim();
            if !(u.starts_with("https://") || u.starts_with("http://")) || u.len() < 12 {
                err.push("[directory]: Verzeichnis braucht die öffentliche Adresse der Station ([station] url = \"https://…\")".into());
            }
            if !(self.directory.server.starts_with("https://") || self.directory.server.starts_with("http://")) {
                err.push("[directory]: server muss mit https:// beginnen".into());
            }
            if !self.sdrs.iter().any(|b| b.enabled && b.guest && !b.admin_only) {
                warn.push("[directory]: kein öffentliches Band – das Verzeichnis zeigt die Station ohne Bänder".into());
            }
        }
        (err, warn)
    }

    fn apply_env_overrides(&mut self) {
        if let Ok(v) = std::env::var("PORT") { if let Ok(v) = v.parse() { self.port = v; } }
        if let Ok(v) = std::env::var("PLUGIN_DIR") { self.plugin_dir = PathBuf::from(v); }
        if let Ok(v) = std::env::var("FRONTEND_DIR") { self.frontend_dir = PathBuf::from(v); }
        if let Ok(v) = std::env::var("DATA_DIR") { self.data_dir = PathBuf::from(v); }

    }

    /// Datei für Änderungen über die Admin-Schnittstelle: die gelesene Konfiguration (Kommentare gehen dabei verloren)
    pub fn config_path(&self) -> PathBuf {
        self.source.clone().unwrap_or_else(|| self.data_dir.join("config.toml"))
    }

    pub fn save(&self, path: &Path) -> Result<(), crate::CrabSdrError> {
        let content = toml::to_string_pretty(self)
            .map_err(|e| crate::CrabSdrError::Config(e.to_string()))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, content)?;
        info!("Config saved to {}", path.display());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load_str(name: &str, s: &str) -> Result<Loaded, String> {
        let p = std::env::temp_dir().join(format!("crabsdr-test-{}-{}.toml", std::process::id(), name));
        std::fs::write(&p, s).unwrap();
        let r = ServerConfig::load_checked(Some(&p));
        let _ = std::fs::remove_file(&p);
        r
    }

    #[test]
    fn neue_und_alte_namen_gleich() {
        let neu = load_str("neu", "[[bands]]\nid = \"2m\"\ndriver = \"rtl_tcp\"\nhost = \"10.0.0.1\"\nport = 6901\ncenter_freq = 145000000\nmode = \"usb\"\nsmeter_cal = -50.0\n").unwrap();
        let alt = load_str("alt", "[smeter_cal]\n\"2m\" = -50.0\n[[sdrs]]\nid = \"2m\"\nsdr_driver = \"rtl_tcp\"\nsdr_tcp_host = \"10.0.0.1\"\nsdr_tcp_port = 6901\ncenter_freq = 145000000\ndefault_mode = \"usb\"\n").unwrap();
        let (a, b) = (&neu.config.sdrs[0], &alt.config.sdrs[0]);
        assert_eq!((a.sdr_driver.as_str(), a.sdr_tcp_host.as_str(), a.sdr_tcp_port, a.default_mode.as_deref(), a.smeter_cal), ("rtl_tcp", "10.0.0.1", 6901, Some("usb"), Some(-50.0)));
        assert_eq!((b.sdr_driver.as_str(), b.sdr_tcp_host.as_str(), b.sdr_tcp_port, b.default_mode.as_deref(), b.smeter_cal), ("rtl_tcp", "10.0.0.1", 6901, Some("usb"), Some(-50.0)));
        assert!(neu.warnings.iter().all(|w| !w.contains("alte")));
        assert!(alt.warnings.iter().any(|w| w.contains("sdrs → bands") && w.contains("[smeter_cal]")));
        assert!(a.guest && a.fft_size == 4096 && a.sample_rate == 2_048_000);
    }

    #[test]
    fn tippfehler_und_fehler() {
        let l = load_str("tipp", "[[bands]]\nid = \"x\"\ncenter_freq = 1\ngian = 3\n").unwrap();
        assert!(l.warnings.iter().any(|w| w.contains("gian")));
        assert!(load_str("syntax", "[[bands]\nid = 1\n").unwrap_err().contains("line 1"));
        assert!(load_str("typ", "port = \"acht\"\n[[bands]]\nid = \"x\"\ncenter_freq = 1\n").is_err());
        assert!(load_str("leer", "port = 80\n").unwrap_err().contains("kein Band"));
        assert!(ServerConfig::load_checked(Some(Path::new("/gibt/es/nicht.toml"))).is_err());
    }

    #[test]
    fn pruefung_decoder_im_band() {
        let l = load_str("dec", "[[bands]]\nid = \"2m\"\ncenter_freq = 145000000\n[[decoders]]\nplugin = \"aprs\"\nfreq = 144800000\n[[decoders]]\nplugin = \"aprs\"\nfreq = 432000000\n").unwrap();
        let (err, _) = l.config.validate();
        assert_eq!(err.len(), 1);
        assert!(err[0].contains("432"));
        assert!(!l.config.db_path.as_os_str().is_empty() && !l.config.frontend_dir.as_os_str().is_empty());
    }

    #[test]
    fn verzeichnis() {
        let band = "[[bands]]\nid = \"2m\"\ncenter_freq = 145000000\n";
        let aus = load_str("dir-aus", band).unwrap();
        assert!(!aus.config.directory.enabled && aus.config.directory.server == "https://crabsdr.de");
        let ohne = load_str("dir-ohne", &format!("[directory]\nenabled = true\n{}", band)).unwrap();
        assert!(ohne.config.validate().0.iter().any(|e| e.contains("öffentliche Adresse")), "ohne station.url kein Eintrag");
        let gut = load_str("dir-gut", &format!("[station]\nurl = \"https://sdr.example.org\"\n[directory]\nenabled = true\n{}", band)).unwrap();
        assert!(gut.config.validate().0.is_empty());
        let intern = load_str("dir-mb", &format!("[station]\nurl = \"https://sdr.example.org\"\n[directory]\nenabled = true\n{}guest = false\n", band)).unwrap();
        assert!(intern.config.validate().1.iter().any(|w| w.contains("kein öffentliches Band")));
    }

    #[test]
    fn rohformat_rx_sdr() {
        let ok = load_str("fmt-ok", "[[bands]]\nid = \"hf\"\ndriver = \"rx_sdr\"\ndevice = \"driver=soapyMiri\"\ncenter_freq = 7100000\nformat = \"cs16\"\n").unwrap();
        assert!(ok.config.validate().0.is_empty() && ok.config.sdrs[0].format.as_deref() == Some("cs16"));
        let falsch = load_str("fmt-bad", "[[bands]]\nid = \"hf\"\ndriver = \"rx_sdr\"\ncenter_freq = 7100000\nformat = \"s12\"\n").unwrap();
        assert!(falsch.config.validate().0.iter().any(|e| e.contains("format")));
        let rtl = load_str("fmt-rtl", "[[bands]]\nid = \"2m\"\ncenter_freq = 145000000\nformat = \"cs16\"\n").unwrap();
        assert!(rtl.config.validate().1.iter().any(|w| w.contains("nur für driver")));
    }

    #[test]
    fn fft_automatisch() {
        assert_eq!((auto_fft_size(2_048_000), auto_fft_size(3_000_000), auto_fft_size(8_000_000)), (4096, 8192, 16384));
        let l = load_str("fft", "[[bands]]\nid = \"a\"\ncenter_freq = 145000000\n[[bands]]\nid = \"b\"\ncenter_freq = 147000000\nsample_rate = 8000000\n[[bands]]\nid = \"c\"\ncenter_freq = 1\nfft_size = 2048\n[[bands]]\nid = \"d\"\ncenter_freq = 1\nfft_size = 3000\n").unwrap();
        let f: Vec<usize> = l.config.sdrs.iter().map(|b| b.fft_size).collect();
        assert_eq!(f, vec![4096, 16384, 2048, 4096]);
        assert!(l.warnings.iter().any(|w| w.contains("3000")));
    }

    #[test]
    fn locator() {
        let (la, lo) = locator_to_latlon("JO53RB").unwrap();
        assert!((la - 53.0625).abs() < 1e-6 && (lo - 11.4583).abs() < 1e-3);
        assert!(locator_to_latlon("ZZ99").is_none());
    }
}
