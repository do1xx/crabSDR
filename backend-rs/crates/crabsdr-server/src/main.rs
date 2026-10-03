//! crabSDR — Multi-SDR / Multi-User WebSDR Server.
//!
//! Each SDR device gets its own pipeline (subprocess + DSP thread + clients).
//! Users connect to a specific band via /ws/{sdr_id}.
//! Auth via JWT + SQLite.

mod access;
mod admin;
mod auth;
mod confedit;
mod ratelimit;
mod chat;
mod check;
mod digi;
mod client;
mod decoders;
mod directory;
mod dsp_thread;
mod sdr_manager;
mod sdr_pipeline;

use axum::{
    extract::{
        ws::{Message, WebSocket},
        Path as AxumPath, Query, State, WebSocketUpgrade,
    },
    http::HeaderMap,
    response::{IntoResponse, Json},
    routing::{get, post, put},
    Router,
};
use crabsdr_core::{protocol, AgcMode, DemodMode, ServerConfig};
use client::{Squelch, SquelchMode, WaterfallSub};
use sdr_manager::SdrManager;
use sdr_pipeline::SdrPipeline;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tower_http::services::ServeDir;
use tracing::{info, warn};

/// Version + Git-Stand des Builds (CRABSDR_GIT setzt packaging/build-binaries.sh bzw. der Aufrufer)
pub fn version_long() -> String {
    match option_env!("CRABSDR_GIT").filter(|g| !g.is_empty()) { Some(g) => format!("{} ({})", env!("CARGO_PKG_VERSION"), g), None => env!("CARGO_PKG_VERSION").to_string() }
}

const USAGE: &str = "crabSDR – WebSDR-Server

Aufruf:  crabsdr-server [--check] [KONFIGURATION]
  KONFIGURATION   Pfad zur config.toml (sonst $CRABSDR_CONFIG, /etc/crabsdr/config.toml, /data/config.toml, ./config.toml)
  --check         Konfiguration prüfen (Syntax, Bänder, Decoder, Pfade, Programme) und beenden; Rückgabe 0 = in Ordnung
  --reset-admin   Konto „admin“ bekommt ein neues Zufallspasswort (wird ausgegeben), alle seine Sitzungen enden
  --version       Version ausgeben
Beschreibung aller Einstellungen: docs/CONFIG.md";

#[tokio::main]
async fn main() {
    let _ = SERVER_START.set(std::time::Instant::now());
    let (mut check, mut reset_admin, mut cfg_path) = (false, false, None::<std::path::PathBuf>);
    for a in std::env::args().skip(1) {
        match a.as_str() {
            "--version" | "-V" => { println!("crabsdr-server {}", version_long()); return; }
            "--help" | "-h" => { println!("{}", USAGE); return; }
            "--check" => check = true,
            "--reset-admin" => reset_admin = true,
            x if x.starts_with('-') => { eprintln!("unbekannte Option {}\n\n{}", x, USAGE); std::process::exit(2); }
            x => cfg_path = Some(x.into()),
        }
    }
    if check { std::process::exit(check::run(cfg_path.as_deref())); }
    if reset_admin { std::process::exit(reset_admin_cli(cfg_path.as_deref())); }
    // Farbcodes nur im Terminal; im Journal/in Dateien wären sie Zeichensalat (und machen syslog größer)
    use std::io::IsTerminal;
    tracing_subscriber::fmt()
        .with_ansi(std::io::stdout().is_terminal())
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".parse().unwrap()),
        )
        .init();

    info!("crabSDR {} startet", version_long());

    let config = match ServerConfig::load_checked(cfg_path.as_deref()) {
        Ok(l) => {
            if let Some(p) = &l.config.source { info!("Konfiguration: {}", p.display()); }
            for w in &l.warnings { warn!("Konfiguration: {}", w); }
            let (errs, warns) = l.config.validate();
            for w in &warns { warn!("Konfiguration: {}", w); }
            for e in &errs { tracing::error!("Konfiguration: {}", e); }
            l.config
        }
        Err(e) => { eprintln!("crabSDR: {}", e); tracing::error!("{}", e); std::process::exit(2); }
    };
    info!(
        "Config: {} SDR(s), port {}, plugins={}",
        config.sdrs.len(),
        config.port,
        config.plugin_dir.display()
    );

    // Start all SDR pipelines
    let mut manager = SdrManager::new();
    manager
        .start_all(config.sdrs.clone(), config.opus_bitrate, config.opus_complexity)
        .await;

    // Decoder-Einsätze ([[decoders]]) an die laufenden Bänder hängen
    let decoder_hub = decoders::DecoderHub::new(config.data_dir.clone());
    decoder_hub.set_station(&config.station);
    decoder_hub.start(&config.decoders, &config.plugin_dir, manager.all()).await;
    if let Some(m) = &config.mqtt { decoder_hub.start_mqtt(m); }

    // Konten und Sitzungen (docs/SECURITY.md)
    let auth_state = match crabsdr_auth::AuthDb::open(&config.db_path) {
        Ok(db) => {
            match db.bootstrap_admin() {
                Ok(Some(pw)) => {
                    info!("**************************************************************");
                    info!("*** Erster Start: Admin-Konto „admin“, Passwort: {} ***", pw);
                    info!("*** Anmelden unter /admin/ – das Passwort muss dort geändert werden ***");
                    info!("**************************************************************");
                }
                Ok(None) => {}
                Err(e) => warn!("Admin-Konto nicht angelegt: {}", e),
            }
            tokio::task::spawn_blocking(crabsdr_auth::warm_up);
            Some(auth::AuthState {
                db: Arc::new(std::sync::Mutex::new(db)),
                jwt: crabsdr_auth::JwtManager::load_or_generate(&config.jwt_secret_env, &config.data_dir),
                limiter: ratelimit::LoginLimiter::default(),
            })
        }
        Err(e) => {
            warn!("Benutzerdatenbank {} nicht zu öffnen: {} – nur öffentliche Bänder, keine Anmeldung, keine Admin-Seite", config.db_path.display(), e);
            None
        }
    };

    let shared = Arc::new(AppState {
        config: Arc::new(RwLock::new(config.clone())),
        manager: Arc::new(RwLock::new(manager)),
        auth: auth_state,
        chat: Arc::new(if config.builtin_chat { chat::Chat::open(&config.data_dir, config.station.home(), config.chat_keep_hours) } else { chat::Chat::disabled() }),
        decoders: decoder_hub,
        config_lock: tokio::sync::Mutex::new(()),
        restart_pending: std::sync::atomic::AtomicBool::new(false),
    });

    // Verbund-Chat über crabsdr.de (nur mit eingebautem Chat und Verzeichnis-Eintrag)
    if config.chat_verbund && config.builtin_chat && config.directory.enabled { tokio::spawn(chat::run_verbund(shared.clone())); }
    // Öffentliches Verzeichnis (crabsdr.de): nur wenn [directory] enabled
    if config.directory.enabled {
        info!("Verzeichnis: Station wird bei {} gelistet (öffentliche Adresse {})", config.directory.server, config.station.url);
        tokio::spawn(directory::run(shared.clone()));
    }

    // Admin-Schnittstelle: eigene Grenzen und Sicherheitsköpfe (docs/SECURITY.md)
    let admin_api = Router::new()
        .route("/api/admin/overview", get(admin::overview))
        .route("/api/admin/users", get(admin::list_users).post(admin::create_user))
        .route("/api/admin/users/{id}", put(admin::update_user).delete(admin::delete_user))
        .route("/api/admin/users/{id}/password", post(admin::reset_password))
        .route("/api/admin/config", get(admin::get_config).put(admin::save_config))
        .route("/api/admin/config/check", post(admin::check_config))
        .route("/api/admin/config/backups/{name}", get(admin::get_backup))
        .route("/api/admin/restart", post(admin::restart))
        .route("/api/admin/bands/{id}/gain", post(admin::live_gain))
        .route("/api/admin/chat", get(admin::list_chat))
        .route("/api/admin/chat/{id}", axum::routing::delete(admin::delete_chat))
        .route("/api/admin/log/{id}", axum::routing::delete(admin::delete_log))
        .route("/api/admin/devices", get(admin::devices))
        .route("/api/admin/devices/serial", post(admin::set_serial))
        .layer(axum::extract::DefaultBodyLimit::max(confedit::MAX_BYTES * 2 + 64 * 1024));
    let auth_api = Router::new()
        .route("/api/auth/login", post(auth::login))
        .route("/api/auth/password", post(auth::change_password))
        .route("/api/auth/me", get(auth::me))
        .route("/api/auth/guest-available", get(auth::guest_available))
        .route("/api/auth/guest", post(auth::guest_login))
        .layer(axum::extract::DefaultBodyLimit::max(16 * 1024));
    // Admin-Seite nur aus der Oberfläche (frontend_dir/admin), nie aus dem Stationsordner
    let admin_page = Router::new()
        .route("/admin", get(|| async { axum::response::Redirect::permanent("admin/") }))
        .nest_service("/admin/", ServeDir::new(config.frontend_dir.join("admin")).append_index_html_on_directories(true));

    let app = Router::new()
        .route("/ws/{sdr_id}", get(ws_handler))
        .route("/api/bands", get(api_list_bands))
        .route("/api/health", get(api_health))
        .route("/api/listeners", get(api_listeners))
        .route("/api/directory", get(directory::api_entry))
        .route("/api/decoders", get(decoders::api_list))
        .route("/api/decoders/events", get(decoders::api_events))
        .route("/api/decoders/files/{id}/{*path}", get(decoders::api_file))
        .route("/bandinfo.js", get(bandinfo_js))
        .route("/ui.json", get(ui_json))
        .route("/", get(index_html))
        .route("/index.html", get(index_html))
        .route("/status.json", get(status_json))
        .route("/gains.json", get(gains_json))
        .route("/digi/aprs.json", get(digi::aprs))
        .route("/digi/relais.json", get(digi::relais))
        .route("/digi/ft8.json", get(digi::ft8))
        .route("/digi/sstv.json", get(digi::sstv))
        .merge(auth_api)
        .merge(admin_api)
        .merge(admin_page)
        // Chat und Logbuch der Hörer (abschaltbar mit builtin_chat = false, z. B. wenn ein eigener Dienst sie übernimmt)
        .merge(if config.builtin_chat { chat::routes() } else { Router::new() })
        .merge(chat::routes_always())
        .fallback_service({
            // Stationsordner hat Vorrang (Logo, Listen, Zusatzseiten, notfalls jede Datei); fehlt eine Datei dort, kommt sie
            // aus der Oberfläche. Ohne site_dir zeigt die erste Stufe auf einen Ordner, den es nicht gibt.
            let web = ServeDir::new(&config.frontend_dir).append_index_html_on_directories(true);
            let site = config.site_dir.clone().unwrap_or_else(|| config.frontend_dir.join(".kein-stationsordner"));
            ServeDir::new(site).append_index_html_on_directories(true).fallback(web)
        })
        .layer(axum::middleware::from_fn(security_headers))
        .with_state(shared);

    let addr = format!("0.0.0.0:{}", config.port);
    info!("Listening on http://{}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap();
}

/// Gemeinsamer Zustand (immer als Arc<AppState> geteilt)
pub struct AppState {
    pub config: Arc<RwLock<ServerConfig>>,
    pub manager: Arc<RwLock<SdrManager>>,
    pub auth: Option<auth::AuthState>,
    pub chat: Arc<chat::Chat>,
    pub decoders: Arc<decoders::DecoderHub>,
    /// Konfiguration schreiben: immer nur einer
    pub config_lock: tokio::sync::Mutex<()>,
    /// Konfiguration geändert, Neustart steht aus
    pub restart_pending: std::sync::atomic::AtomicBool,
}

pub fn uptime_s() -> u64 { SERVER_START.get().map(|t| t.elapsed().as_secs()).unwrap_or(0) }

/// `--reset-admin`: am Rechner, mit Zugang zur Datenbank (Benutzer crabsdr oder root)
fn reset_admin_cli(path: Option<&std::path::Path>) -> i32 {
    let cfg = match ServerConfig::load_checked(path) { Ok(l) => l.config, Err(e) => { eprintln!("crabSDR: {}", e); return 2; } };
    match crabsdr_auth::AuthDb::open(&cfg.db_path).and_then(|db| db.reset_admin()) {
        Ok(pw) => {
            println!("Konto „admin“: neues Passwort {}", pw);
            println!("Anmelden unter /admin/ – dort muss es sofort geändert werden. Alle bisherigen Sitzungen sind beendet.");
            0
        }
        Err(e) => { eprintln!("crabSDR: Benutzerdatenbank {}: {}", cfg.db_path.display(), e); 1 }
    }
}

// Make AuthState cloneable via Arc internals

// --- WebSocket Handler ---

/// WebSocket je Band. Ohne Token = Gast; ungültiges oder widerrufenes Token = 401 (die Seite meldet sich dann neu an).
async fn ws_handler(
    ws: WebSocketUpgrade,
    AxumPath(sdr_id): AxumPath<String>,
    Query(q): Query<TokenQuery>,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let pipeline = match state.manager.read().await.get(&sdr_id) {
        Some(p) => p.clone(),
        None => return (axum::http::StatusCode::NOT_FOUND, "Band gibt es nicht").into_response(),
    };
    let p = match listener_principal(&state, &headers, &q).await {
        Ok(p) => p,
        Err(()) => return (axum::http::StatusCode::UNAUTHORIZED, "Sitzung abgelaufen").into_response(),
    };
    let (public, admin_only) = (pipeline.guest.load(std::sync::atomic::Ordering::Relaxed), pipeline.admin_only.load(std::sync::atomic::Ordering::Relaxed));
    if !p.may_band(&sdr_id, public, admin_only) {
        return (axum::http::StatusCode::FORBIDDEN, "Band nur für angemeldete Hörer").into_response();
    }
    let (is_admin, role) = (p.is_admin(), p.role.clone());
    ws.on_upgrade(move |socket| handle_ws(socket, pipeline, is_admin, role)).into_response()
}

async fn handle_ws(mut socket: WebSocket, pipeline: Arc<SdrPipeline>, is_admin: bool, role: String) {
    static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let client_id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    let (audio_tx, mut audio_rx) = mpsc::channel::<Vec<u8>>(32);

    {
        let mut clients = pipeline.clients.lock().await;
        clients.add(client_id, audio_tx);
    }

    info!("[{}] Client {} connected (admin={})", pipeline.id, client_id, is_admin);

    // Send initial config
    {
        let current_center = crate::sdr_pipeline::corrected_center(pipeline.center_freq.load(std::sync::atomic::Ordering::Relaxed), pipeline.corr_ppm);
        let config_msg = json!({
            "type": "config",
            "client_id": client_id,
            "sdr_id": pipeline.id,
            "label": pipeline.label,
            "center_freq": current_center,
            "sample_rate": pipeline.sample_rate.load(std::sync::atomic::Ordering::Relaxed),
            "fft_size": pipeline.fft_size,
            "admin_only": pipeline.admin_only.load(std::sync::atomic::Ordering::Relaxed),
            "is_admin": is_admin,
            "role": role,
            "default_mode": pipeline.config.read().await.default_mode,
            "modes": DemodMode::all().iter().map(|m| m.as_str()).collect::<Vec<_>>(),
        });
        let _ = socket
            .send(Message::Text(config_msg.to_string().into()))
            .await;
    }

    let mut spectrum_rx = pipeline.spectrum_tx.subscribe();

    loop {
        tokio::select! {
            Ok(frame) = spectrum_rx.recv() => {
                // Broadcast trägt nur noch JSON (Decoder, Statistik, Hörerliste); Ton und Wasserfall kommen je Hörer
                if frame.first() == Some(&protocol::TAG_JSON)
                    && socket.send(Message::Binary(frame.into())).await.is_err() {
                        break;
                    }
            }

            Some(audio_frame) = audio_rx.recv() => {
                if socket.send(Message::Binary(audio_frame.into())).await.is_err() {
                    break;
                }
            }

            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        handle_client_message(client_id, &text, &pipeline, is_admin).await;
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
        }
    }

    {
        let mut clients = pipeline.clients.lock().await;
        clients.remove(client_id);
    }
    info!("[{}] Client {} disconnected", pipeline.id, client_id);
}

async fn handle_client_message(client_id: u64, text: &str, pipeline: &SdrPipeline, is_admin: bool) {
    let msg: serde_json::Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => {
            warn!(
                "[{}] Client {} sent invalid JSON: {}",
                pipeline.id, client_id, e
            );
            return;
        }
    };

    let cmd_type = msg.get("type").and_then(|v| v.as_str()).unwrap_or("");

    match cmd_type {
        "tune" => {
            let freq = msg.get("freq").and_then(|v| v.as_u64()).unwrap_or(0);
            let mode_str = msg
                .get("mode")
                .and_then(|v| v.as_str())
                .unwrap_or("wfm");
            let mode = DemodMode::from_str(mode_str).unwrap_or(DemodMode::Wfm);
            let bandwidth = msg
                .get("bandwidth")
                .and_then(|v| v.as_u64())
                .map(|v| v as u32)
                .unwrap_or_else(|| mode.default_bandwidth());

            // SSB: untere Kante des Durchlassbereichs (Browser schickt lo in Hz, Voreinstellung 300)
            let pass_lo = msg.get("lo").and_then(|v| v.as_u64()).map(|v| v.min(5000) as u32).unwrap_or(300);
            let mut clients = pipeline.clients.lock().await;
            clients.update_tune_lo(client_id, freq, mode, bandwidth, pass_lo);

            info!(
                "[{}] Client {} tuned to {} Hz ({})",
                pipeline.id,
                client_id,
                freq,
                mode.as_str()
            );
        }
        "set_gain" => {
            if !is_admin || !pipeline.admin_only.load(std::sync::atomic::Ordering::Relaxed) {
                warn!("[{}] Client {} tried set_gain (admin={}, admin_only={})", pipeline.id, client_id, is_admin, pipeline.admin_only.load(std::sync::atomic::Ordering::Relaxed));
                return;
            }
            if let Some(gain) = msg.get("gain").and_then(|v| v.as_f64()) {
                let _ = pipeline
                    .sdr_cmd_tx
                    .send(crabsdr_sdr::DriverCommand::SetGain(gain))
                    .await;
                info!("[{}] Admin set gain to {}", pipeline.id, gain);
            }
        }
        "set_center_freq" => {
            if !is_admin || !pipeline.admin_only.load(std::sync::atomic::Ordering::Relaxed) {
                warn!("[{}] Client {} tried set_center_freq (admin={}, admin_only={})",
                    pipeline.id, client_id, is_admin, pipeline.admin_only.load(std::sync::atomic::Ordering::Relaxed));
                return;
            }
            if let Some(freq) = msg.get("freq").and_then(|v| v.as_u64()) {
                pipeline.center_freq.store(freq, std::sync::atomic::Ordering::Relaxed);
                let _ = pipeline
                    .sdr_cmd_tx
                    .send(crabsdr_sdr::DriverCommand::SetFrequency(freq))
                    .await;
                // Broadcast update to all clients
                let shown = crate::sdr_pipeline::corrected_center(freq, pipeline.corr_ppm);
                pipeline.broadcast_json(&json!({
                    "type": "center_freq_update",
                    "center_freq": shown,
                }));
                info!("[{}] Admin set center_freq to {} Hz", pipeline.id, freq);
            }
        }
        "set_codec" => {
            let audio = msg.get("audio").and_then(|v| v.as_str()).unwrap_or("raw");
            let use_opus = audio == "opus";
            let mut clients = pipeline.clients.lock().await;
            clients.set_opus(client_id, use_opus);
        }
        "untune" => {
            let mut clients = pipeline.clients.lock().await;
            clients.untune(client_id);
        }
        "set_squelch" => {
            // {"type":"set_squelch","mode":"off|auto|manual","db":-60,"margin":6,"hang_ms":500}
            let mode = match msg.get("mode").and_then(|v| v.as_str()).unwrap_or("off") {
                "auto" => SquelchMode::Auto,
                "manual" => SquelchMode::Manual,
                _ => SquelchMode::Off,
            };
            let db = msg.get("db").and_then(|v| v.as_f64()).unwrap_or(-60.0) as f32;
            let margin_db = msg.get("margin").and_then(|v| v.as_f64()).map(|m| m.clamp(0.0, 40.0) as f32).unwrap_or(client::AUTO_SQUELCH_MARGIN_DB);
            let hang_ms = msg.get("hang_ms").and_then(|v| v.as_u64()).unwrap_or(500).min(10_000) as u32;
            let mut clients = pipeline.clients.lock().await;
            clients.set_squelch(client_id, Squelch { mode, db, margin_db, hang_ms });
        }
        "set_name" => {
            let name = msg.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let mut clients = pipeline.clients.lock().await;
            clients.set_name(client_id, name);
        }
        "session" => {
            // Sitzungskennung des Browsers: mehrere Band-Verbindungen derselben Person zählen als ein Hörer
            let sid = msg.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let mut clients = pipeline.clients.lock().await;
            clients.set_session(client_id, sid);
        }
        "set_waterfall" => {
            // {"type":"set_waterfall","full":true}  (altes Vollspektrum)  oder
            // {"type":"set_waterfall","zoom":0,"start_bin":0}  (1024-px-Zeile, Zoomstufe 0..2)  oder {"off":true}
            let mut clients = pipeline.clients.lock().await;
            if let Some(rows) = msg.get("hist_rows").and_then(|v| v.as_u64()) {
                let slow = msg.get("slow").and_then(|v| v.as_u64()).unwrap_or(1).clamp(1, 16) as u8;
                let jpeg = msg.get("fmt").and_then(|v| v.as_str()) == Some("jpeg");
                let mode = msg.get("mode").and_then(|v| v.as_u64()).unwrap_or(1).min(3) as u8;
                clients.set_waterfall_hist(client_id, rows.min(2000) as u16, slow, jpeg, mode);
            }
            if msg.get("off").and_then(|v| v.as_bool()).unwrap_or(false) {
                clients.set_waterfall(client_id, None);
            } else if let Some(zoom) = msg.get("zoom").and_then(|v| v.as_u64()) {
                // bis Stufe srv_max ist ein Pixel ein Bin des Gesamtspektrums, darüber (5 Stufen) rechnet der Server ein Zoom-Spektrum
                let srv_max = ((pipeline.fft_size as u64 / 1024).max(1)).ilog2() as u64;
                let zoom = zoom.min(srv_max + 5) as u8;
                let max_start = (pipeline.fft_size as u64).saturating_sub((pipeline.fft_size as u64 >> zoom).max(1));   // Ausschnitt = fft/2^zoom Bins
                let start_bin = msg.get("start_bin").and_then(|v| v.as_u64()).unwrap_or(0).min(max_start) as u16;
                clients.set_waterfall(client_id, Some(WaterfallSub { zoom, start_bin }));
            }
        }
        "set_agc" => {
            let mode_str = msg.get("mode").and_then(|v| v.as_str()).unwrap_or("medium");
            if let Some(agc_mode) = AgcMode::from_str(mode_str) {
                let mut clients = pipeline.clients.lock().await;
                clients.set_agc_mode(client_id, agc_mode);
            }
        }
        _ => {
            warn!(
                "[{}] Client {} sent unknown command: {}",
                pipeline.id, client_id, cmd_type
            );
        }
    }
}

// --- REST API ---

#[derive(serde::Deserialize)]
struct TokenQuery { token: Option<String> }

/// Person aus `Authorization: Bearer` oder `?token=` (Hören-Seite: Skripte und WebSockets können keinen Kopf setzen)
async fn listener_principal(state: &AppState, headers: &HeaderMap, q: &TokenQuery) -> Result<access::Principal, ()> {
    let tok = access::bearer(headers).or_else(|| q.token.clone());
    access::resolve(state, tok.as_deref()).await
}

/// Bänder, die diese Person hören darf
async fn api_list_bands(State(state): State<Arc<AppState>>, Query(q): Query<TokenQuery>, headers: HeaderMap) -> Json<serde_json::Value> {
    let p = listener_principal(&state, &headers, &q).await.unwrap_or_else(|_| access::Principal::guest());
    let manager = state.manager.read().await;
    let mut visible = Vec::new();
    for pipe in manager.all().values() {
        if p.may_band(&pipe.id, pipe.guest.load(std::sync::atomic::Ordering::Relaxed), pipe.admin_only.load(std::sync::atomic::Ordering::Relaxed)) {
            visible.push(pipe.band_info().await);
        }
    }
    Json(json!({ "bands": visible }))
}

/// `bandinfo.js` für die Oberfläche: die Bänder, die diese Person hören darf, in Konfigurationsreihenfolge, kHz.
/// Andere Bänder erscheinen gar nicht erst.
async fn bandinfo_js(State(state): State<Arc<AppState>>, Query(q): Query<TokenQuery>, headers: HeaderMap) -> impl IntoResponse {
    let p = listener_principal(&state, &headers, &q).await.unwrap_or_else(|_| access::Principal::guest());
    let config = state.config.read().await;
    let manager = state.manager.read().await;
    let mut entries = Vec::new();
    for sdr in config.sdrs.iter().filter(|s| s.enabled && p.may_band(&s.id, s.guest, s.admin_only)) {
        let (center, rate) = match manager.get(&sdr.id) {
            Some(p) => (p.center_freq.load(std::sync::atomic::Ordering::Relaxed), p.sample_rate.load(std::sync::atomic::Ordering::Relaxed)),
            None => (sdr.center_freq, sdr.sample_rate),
        };
        let center = crate::sdr_pipeline::corrected_center(center, sdr.freq_correction_ppm);
        entries.push(json!({"name": sdr.id, "label": sdr.label, "centerfreq": center as f64 / 1000.0,
                            "samplerate": rate as f64 / 1000.0, "fft_size": sdr.fft_size, "note": sdr.note,
                            "access": admin::access_name(sdr.guest, sdr.admin_only),
                            "mode": sdr.default_mode.as_ref().map(|m| m.to_lowercase())}));
    }
    let n = entries.len();
    let body = format!("var bandinfo = {};\nvar nbands = {};\n", serde_json::Value::Array(entries), n);
    ([(axum::http::header::CONTENT_TYPE, "application/javascript; charset=utf-8"), (axum::http::header::CACHE_CONTROL, "no-store")], body)
}

static SERVER_START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

/// Datei der Oberfläche: zuerst aus dem Stationsordner (site_dir), sonst aus frontend_dir.
async fn read_ui_file(state: &AppState, name: &str) -> Option<Vec<u8>> {
    let (site, web) = { let c = state.config.read().await; (c.site_dir.clone(), c.frontend_dir.clone()) };
    if let Some(s) = site { if let Ok(b) = tokio::fs::read(s.join(name)).await { return Some(b); } }
    tokio::fs::read(web.join(name)).await.ok()
}
async fn ui_file_exists(state: &AppState, name: &str) -> bool {
    let (site, web) = { let c = state.config.read().await; (c.site_dir.clone(), c.frontend_dir.clone()) };
    site.is_some_and(|s| s.join(name).exists()) || web.join(name).exists()
}

/// `status.json` für die Statuszeile: Datei aus dem Frontend-Verzeichnis (z. B. von einem Wächter geschrieben), sonst
/// selbst erzeugt (Laufzeit, Last, CPU-Temperatur soweit lesbar).
async fn status_json(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let hdr = [(axum::http::header::CONTENT_TYPE, "application/json"), (axum::http::header::CACHE_CONTROL, "no-store")];
    if let Some(b) = read_ui_file(&state, "status.json").await { return (hdr, b).into_response(); }
    // Laufzeit des Servers selbst (im Container wäre /proc/uptime die der Docker-VM)
    let up = SERVER_START.get().map(|t| t.elapsed().as_secs_f64()).unwrap_or(0.0);
    let load = std::fs::read_to_string("/proc/loadavg").ok().and_then(|s| s.split_whitespace().next().map(|v| v.to_string())).unwrap_or_default();
    let temp = std::fs::read_to_string("/sys/class/thermal/thermal_zone0/temp").ok().and_then(|s| s.trim().parse::<i64>().ok()).map(|t| t / 1000);
    let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let mut v = json!({"ts": ts, "up_s": up as u64, "load": load});
    if let Some(t) = temp { v["temp"] = json!(t); }
    (hdr, v.to_string()).into_response()
}

/// `gains.json` fürs S-Meter (dBm = Rohwert − rtl_gain − ws_gain + cal): Datei aus dem Frontend-Verzeichnis ( vom Wächter), sonst aus der Konfiguration – die Stick-Verstärkung je Band, damit `smeter_cal` bei 0 dB gilt und eine
/// geänderte Verstärkung die Anzeige nicht verstellt.
async fn gains_json(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let (_dir, bands) = {
        let c = state.config.read().await;
        let b: serde_json::Map<String, serde_json::Value> = c.sdrs.iter().filter(|s| s.enabled)
            .map(|s| (s.id.clone(), json!({"rtl_gain": if s.sdr_driver.starts_with("rtl") { s.gain } else { 0.0 }, "ws_gain": 0.0, "src": s.sdr_driver})))
            .collect();
        (c.frontend_dir.clone(), b)
    };
    let hdr = [(axum::http::header::CONTENT_TYPE, "application/json"), (axum::http::header::CACHE_CONTROL, "no-store")];
    if let Some(b) = read_ui_file(&state, "gains.json").await { return (hdr, b).into_response(); }
    (hdr, json!({"bands": bands}).to_string()).into_response()
}

/// `index.html` aus dem Frontend-Verzeichnis; Platzhalter `{{STATION_*}}` werden aus `[station]` der Konfiguration
/// ersetzt (neutrale Oberfläche des Pakets). Seiten ohne Platzhalter bleiben unverändert.
async fn index_html(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let st = state.config.read().await.station.clone();
    let html = match read_ui_file(&state, "index.html").await.and_then(|b| String::from_utf8(b).ok()) {
        Some(h) => h,
        None => return (axum::http::StatusCode::NOT_FOUND, "index.html fehlt im Frontend-Verzeichnis").into_response(),
    };
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;");
    // Name mit abgesetztem Schlusswort im Kopf, z. B. „DL0XYZ <span>WebSDR</span>“
    let name_html = match st.name.rsplit_once(' ') {
        Some((a, b)) if b.eq_ignore_ascii_case("websdr") || b.eq_ignore_ascii_case("sdr") => format!("{} <span>{}</span>", esc(a), esc(b)),
        _ => esc(&st.name),
    };
    let name_js = serde_json::to_string(&st.name).unwrap_or_default().trim_matches('"').replace("</", "<\\/").replace('\'', "\\'");
    let out = html.replace("{{STATION_NAME_HTML}}", &name_html).replace("{{STATION_NAME_JS}}", &name_js)
        .replace("{{STATION_NAME}}", &esc(&st.name)).replace("{{STATION_SUB}}", &esc(&st.subtitle))
        .replace("{{STATION_URL}}", &esc(&st.url)).replace("{{STATION_LOCATOR}}", &esc(&st.locator));
    ([(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8"), (axum::http::header::CACHE_CONTROL, "no-cache")], out).into_response()
}

/// `ui.json` aus dem Frontend-Verzeichnis, `smeter.cal` je Band aus der eigenen Konfiguration überlagert
/// (Rohwert = dBFS).
async fn ui_json(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let has_private = { let c = state.config.read().await; c.sdrs.iter().any(|b| b.enabled && (!b.guest || b.admin_only)) } && state.auth.is_some();
    let (cal, ui, has_decoders, chat_on, station) = { let c = state.config.read().await;
        let cal: std::collections::HashMap<String, f64> = c.sdrs.iter().filter_map(|b| b.smeter_cal.map(|v| (b.id.clone(), v))).collect();
        (cal, c.ui.clone(), c.decoders.iter().any(|d| d.enabled && d.public), c.builtin_chat, c.station.clone()) };
    let mut v: serde_json::Value = match read_ui_file(&state, "ui.json").await.and_then(|b| serde_json::from_slice(&b).ok()) {
        Some(v) => v, None => json!({}),
    };
    // Schalter der Oberfläche: ausdrücklich aus [ui], sonst Voreinstellung (Zusatzseiten nur, wenn es sie gibt)
    let verbund_on = { let c = state.config.read().await; c.chat_verbund && c.builtin_chat && c.directory.enabled };
    let features = json!({
        "chat": ui.chat.unwrap_or(chat_on), "status": ui.status.unwrap_or(true), "recording": ui.recording.unwrap_or(true),
        "decoders": ui.decoders.unwrap_or(has_decoders),
        // Digital nur mit öffentlichen Decodern, Logbuch nur mit eingebautem Chat/Logbuch-Dienst (oder ausdrücklich per [ui])
        "digital": match ui.digital { Some(b) => b, None => has_decoders && ui_file_exists(&state, "digi/index.html").await },
        "logbook": match ui.logbook { Some(b) => b, None => chat_on && ui_file_exists(&state, "logbuch/index.html").await },
        "login": ui.login.unwrap_or(has_private) && state.auth.is_some(),
        "info": match ui.info { Some(b) => b, None => ui_file_exists(&state, "info/index.html").await },
        // eigenes Logo im Stationsordner → crabSDR-Marke klein neben dem Titel, sonst ist die Krabbe selbst das Logo
        "own_logo": state.config.read().await.site_dir.as_ref().is_some_and(|d| d.join("logo.svg").exists()),
        "chat_verbund": verbund_on,
    });
    if let Some(o) = v.as_object_mut() {
        o.insert("features".into(), features);
        o.insert("banner".into(), json!(ui.banner));
        o.insert("version".into(), json!(env!("CARGO_PKG_VERSION")));
        o.insert("build".into(), json!(option_env!("CRABSDR_GIT").unwrap_or("")));
        let home = station.home();
        o.insert("station".into(), json!({"name": station.name, "subtitle": station.subtitle, "locator": station.locator, "url": station.url,
            "lat": home.map(|h| (h.0 * 1e5).round() / 1e5), "lon": home.map(|h| (h.1 * 1e5).round() / 1e5),
            "operator": station.operator, "address": station.address, "contact": station.contact}));
        // Admin-Link (nur bei Aufruf über private Adressen sichtbar): eigene Angabe, sonst die eingebaute Admin-Seite
        let admin_link = ui.admin.clone().or_else(|| state.auth.is_some().then(|| "admin/".to_string()));
        // Impressum/Datenschutz: eigene Links, sonst die Abschnitte auf der Info-Seite (Impressum nur mit Betreiber-Angabe)
        let impressum = ui.impressum.clone().or_else(|| (!station.operator.is_empty()).then(|| "info/#impressum".to_string()));
        let datenschutz = ui.datenschutz.clone().unwrap_or_else(|| "info/#datenschutz".to_string());
        o.insert("links".into(), json!({"impressum": impressum, "datenschutz": datenschutz, "admin": admin_link}));
    }
    if !cal.is_empty() {
        let sm = v.as_object_mut().map(|o| o.entry("smeter").or_insert_with(|| json!({})));
        if let Some(sm) = sm.and_then(|s| s.as_object_mut()) {
            let c = sm.entry("cal").or_insert_with(|| json!({}));
            if let Some(co) = c.as_object_mut() { for (k, val) in cal { co.insert(k, json!(val)); } }
            sm.insert("cal_info".into(), json!("crabSDR: Kalibrierung aus config.toml, smeter_cal je Band (Rohwert = dBFS)"));
        }
    }
    ([(axum::http::header::CONTENT_TYPE, "application/json"), (axum::http::header::CACHE_CONTROL, "no-store")], v.to_string())
}

/// Hörerliste über alle Bänder (Ersatz für ~~othersjj der alten Oberfläche).
/// Hörer = Personen: Verbindungen mit derselben Sitzungskennung (eine Person, mehrere Bänder offen) werden zu einem
/// Eintrag zusammengefasst; steht eine davon auf einer Frequenz, zählt diese. `n` = Personen, `n_audio` = Personen,
/// die gerade hören (Frequenz eingestellt), `connections` = rohe Verbindungen.
async fn api_listeners(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let manager = state.manager.read().await;
    let mut raw = Vec::new();
    for (id, pipeline) in manager.all() {
        let clients = pipeline.clients.lock().await;
        raw.extend(clients.listeners_json(id));
    }
    let connections = raw.len();
    let mut order: Vec<String> = Vec::new();
    let mut by: std::collections::HashMap<String, serde_json::Value> = std::collections::HashMap::new();
    for e in raw {
        let sid = e["session"].as_str().unwrap_or("").to_string();
        let key = if sid.is_empty() { format!("c{}", e["id"]) } else { sid };
        match by.get(&key) {
            Some(cur) if cur["freq"].is_null() || e["freq"].is_null() => { if cur["freq"].is_null() && !e["freq"].is_null() { by.insert(key, e); } }
            Some(_) => {}   // schon ein hörender Eintrag dieser Person
            None => { order.push(key.clone()); by.insert(key, e); }
        }
    }
    let list: Vec<serde_json::Value> = order.iter().filter_map(|k| by.get(k).cloned()).map(|mut e| { if let Some(o) = e.as_object_mut() { o.remove("session"); } e }).collect();
    let n_audio = list.iter().filter(|e| !e["freq"].is_null()).count();
    Json(json!({"listeners": list, "n": list.len(), "n_audio": n_audio, "connections": connections}))
}

async fn api_health(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let manager = state.manager.read().await;
    let mut total_clients = 0;
    for pipeline in manager.all().values() {
        let clients = pipeline.clients.lock().await;
        total_clients += clients.count_real();
    }

    Json(json!({
        "status": "ok",
        "sdr_count": manager.list().len(),
        "total_clients": total_clients,
        "version": version_long(),
        "uptime_s": SERVER_START.get().map(|t| t.elapsed().as_secs()).unwrap_or(0),
    }))
}

/// Sicherheitsköpfe (docs/SECURITY.md): überall nosniff und Referrer-Regel; Admin-Seite und Admin-Schnittstelle
/// zusätzlich strenge CSP, keine Einbettung, kein Zwischenspeichern.
async fn security_headers(req: axum::extract::Request, next: axum::middleware::Next) -> axum::response::Response {
    use axum::http::{header, HeaderValue};
    let path = req.uri().path().to_string();
    let mut res = next.run(req).await;
    let h = res.headers_mut();
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    h.entry(header::REFERRER_POLICY).or_insert(HeaderValue::from_static("strict-origin-when-cross-origin"));
    // Oberfläche nach jedem Update sofort neu: Browser fragen kurz nach (304 ohne Inhalt, wenn unverändert). Ohne diesen
    // Kopf behalten Browser JS/CSS nach eigener Schätzung tagelang und mischen alte Oberfläche mit neuem Server.
    h.entry(header::CACHE_CONTROL).or_insert(HeaderValue::from_static("no-cache"));
    let admin = path == "/admin" || path.starts_with("/admin/") || path.starts_with("/api/admin/") || path.starts_with("/api/auth/");
    if admin {
        h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static(
            "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; font-src 'self'; connect-src 'self'; \
             object-src 'none'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'"));
        h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
        h.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    res
}
