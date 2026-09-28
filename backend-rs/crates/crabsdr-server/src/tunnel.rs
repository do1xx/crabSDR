//! Tunnel client — connects to a central directory server via WSS.
//!
//! When configured with a SiteConfig, CrabSDR establishes an outgoing WebSocket
//! connection to the directory server. The directory server can then proxy browser
//! users to this CrabSDR instance without any port-forwarding or VPN.
//!
//! Protocol:
//!   Text frames = JSON control messages
//!   Binary frames = [session_id: u32 LE][CrabSDR binary payload...]

use crate::sdr_pipeline::SdrPipeline;
use crate::AppState;
use crabsdr_core::{DemodMode, SiteConfig};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Mutex};
use tokio_tungstenite::{connect_async, tungstenite};
use tracing::{debug, error, info, warn};

/// Base ID for tunnel virtual clients (separate range from plugin clients).
const TUNNEL_CLIENT_BASE: u64 = 0xEEEE_0000_0000_0000;

/// Run the tunnel client forever, reconnecting on failures.
pub async fn run_tunnel(site: SiteConfig, state: Arc<AppState>) {
    let mut backoff = Duration::from_secs(2);
    let max_backoff = Duration::from_secs(60);

    loop {
        info!("Tunnel connecting to {} as '{}'...", site.directory_url, site.name);
        match connect_and_run(&site, &state).await {
            Ok(()) => {
                info!("Tunnel disconnected cleanly, reconnecting...");
                backoff = Duration::from_secs(2); // Reset backoff on clean disconnect
            }
            Err(e) => {
                warn!("Tunnel error: {}, reconnecting in {:?}...", e, backoff);
            }
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(max_backoff);
    }
}

/// Single tunnel connection lifecycle.
async fn connect_and_run(
    site: &SiteConfig,
    state: &Arc<AppState>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Connect via WSS
    let (ws_stream, _response) = connect_async(&site.directory_url).await?;
    let (mut ws_tx, mut ws_rx) = ws_stream.split();

    info!("Tunnel connected to {}", site.directory_url);

    // Gather band info for registration
    let bands = {
        let manager = state.manager.read().await;
        manager.band_info_all().await
    };

    // Send registration
    let register_msg = json!({
        "type": "register",
        "token": site.directory_token,
        "site": {
            "name": site.name,
            "locator": site.locator,
        },
        "bands": bands,
    });
    ws_tx
        .send(tungstenite::Message::Text(register_msg.to_string().into()))
        .await?;

    info!("Tunnel registered as '{}' with {} bands", site.name, bands.len());

    // Session tracking: session_id → (pipeline_id, client_id, spectrum_task_handle)
    let sessions: Arc<Mutex<HashMap<u32, SessionState>>> = Arc::new(Mutex::new(HashMap::new()));
    let (tunnel_tx, mut tunnel_rx) = mpsc::channel::<TunnelOutgoing>(64);

    // Heartbeat task
    let heartbeat_tx = tunnel_tx.clone();
    let heartbeat_state = state.clone();
    let heartbeat_handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(30));
        loop {
            interval.tick().await;
            let bands = {
                let manager = heartbeat_state.manager.read().await;
                manager.band_info_all().await
            };
            let msg = json!({
                "type": "heartbeat",
                "bands": bands,
            });
            if heartbeat_tx
                .send(TunnelOutgoing::Text(msg.to_string()))
                .await
                .is_err()
            {
                break;
            }
        }
    });

    // Main loop: multiplex between incoming WS messages, outgoing tunnel data, and tunnel_rx
    loop {
        tokio::select! {
            // Incoming from directory server
            msg = ws_rx.next() => {
                match msg {
                    Some(Ok(tungstenite::Message::Text(text))) => {
                        handle_directory_message(
                            &text,
                            state,
                            &sessions,
                            &tunnel_tx,
                        ).await;
                    }
                    Some(Ok(tungstenite::Message::Binary(data))) => {
                        // Binary from directory = browser data for a session
                        handle_directory_binary(&data, state, &sessions).await;
                    }
                    Some(Ok(tungstenite::Message::Ping(data))) => {
                        let _ = ws_tx.send(tungstenite::Message::Pong(data)).await;
                    }
                    Some(Ok(tungstenite::Message::Close(_))) | None => {
                        info!("Tunnel WebSocket closed");
                        break;
                    }
                    Some(Err(e)) => {
                        error!("Tunnel WebSocket error: {}", e);
                        break;
                    }
                    _ => {}
                }
            }

            // Outgoing from our session handlers
            Some(outgoing) = tunnel_rx.recv() => {
                let result = match outgoing {
                    TunnelOutgoing::Text(text) => {
                        ws_tx.send(tungstenite::Message::Text(text.into())).await
                    }
                    TunnelOutgoing::Binary(data) => {
                        ws_tx.send(tungstenite::Message::Binary(data.into())).await
                    }
                };
                if result.is_err() {
                    error!("Tunnel send failed, disconnecting");
                    break;
                }
            }
        }
    }

    // Cleanup
    heartbeat_handle.abort();

    // Close all sessions
    let mut sessions = sessions.lock().await;
    for (session_id, session) in sessions.drain() {
        cleanup_session(state, session_id, session).await;
    }

    Ok(())
}

/// A virtual client session proxied through the tunnel.
struct SessionState {
    pipeline_id: String,
    client_id: u64,
    /// Handle for the task that forwards spectrum+audio to the tunnel
    forward_handle: tokio::task::JoinHandle<()>,
}

/// Messages to send through the tunnel WebSocket.
enum TunnelOutgoing {
    Text(String),
    Binary(Vec<u8>),
}

/// Handle a JSON control message from the directory server.
async fn handle_directory_message(
    text: &str,
    state: &Arc<AppState>,
    sessions: &Arc<Mutex<HashMap<u32, SessionState>>>,
    tunnel_tx: &mpsc::Sender<TunnelOutgoing>,
) {
    let msg: serde_json::Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => {
            warn!("Tunnel: invalid JSON from directory: {}", e);
            return;
        }
    };

    let msg_type = msg.get("type").and_then(|v| v.as_str()).unwrap_or("");

    match msg_type {
        "session_open" => {
            let session_id = msg.get("session_id").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            let sdr_id = msg
                .get("sdr_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            info!("Tunnel: opening session {} for SDR '{}'", session_id, sdr_id);

            // Find the pipeline
            let manager = state.manager.read().await;
            let pipeline = match manager.get(&sdr_id) {
                Some(p) => p.clone(),
                None => {
                    warn!("Tunnel: session_open for unknown SDR '{}'", sdr_id);
                    let close_msg = json!({
                        "type": "session_close",
                        "session_id": session_id,
                    });
                    let _ = tunnel_tx
                        .send(TunnelOutgoing::Text(close_msg.to_string()))
                        .await;
                    return;
                }
            };
            drop(manager);

            let client_id = TUNNEL_CLIENT_BASE + session_id as u64;
            let (audio_tx, mut audio_rx) = mpsc::channel::<Vec<u8>>(4);

            // Register virtual client
            {
                let mut clients = pipeline.clients.lock().await;
                clients.add(client_id, audio_tx);
            }

            // Subscribe to spectrum
            let mut spectrum_rx = pipeline.spectrum_tx.subscribe();

            // Send initial config to browser via tunnel
            let config_msg = {
                let config = pipeline.config.read().await;
                let current_center =
                    pipeline.center_freq.load(std::sync::atomic::Ordering::Relaxed);
                json!({
                    "type": "config",
                    "sdr_id": pipeline.id,
                    "label": pipeline.label,
                    "center_freq": current_center,
                    "sample_rate": pipeline.sample_rate.load(std::sync::atomic::Ordering::Relaxed),
                    "fft_size": pipeline.fft_size,
                    "admin_only": false,
                    "is_admin": false,
                    "role": "guest",
                    "default_mode": config.default_mode,
                    "modes": crabsdr_core::DemodMode::all().iter().map(|m| m.as_str()).collect::<Vec<_>>(),
                    "plugins": serde_json::Value::Array(vec![]),
                    "channels": serde_json::Value::Array(vec![]),
                })
            };
            // Send config as binary frame (tag 0x03 JSON) with session prefix
            let config_json = config_msg.to_string();
            let config_frame = crabsdr_core::protocol::encode_json(&config_json);
            let mut prefixed = Vec::with_capacity(4 + config_frame.len());
            prefixed.extend_from_slice(&session_id.to_le_bytes());
            prefixed.extend_from_slice(&config_frame);
            let _ = tunnel_tx.send(TunnelOutgoing::Binary(prefixed)).await;

            // Spawn forwarder task: spectrum + audio → tunnel binary frames
            let tx = tunnel_tx.clone();
            let forward_handle = tokio::spawn(async move {
                loop {
                    tokio::select! {
                        Ok(frame) = spectrum_rx.recv() => {
                            let mut prefixed = Vec::with_capacity(4 + frame.len());
                            prefixed.extend_from_slice(&session_id.to_le_bytes());
                            prefixed.extend_from_slice(&frame);
                            if tx.send(TunnelOutgoing::Binary(prefixed)).await.is_err() {
                                break;
                            }
                        }
                        Some(audio_frame) = audio_rx.recv() => {
                            let mut prefixed = Vec::with_capacity(4 + audio_frame.len());
                            prefixed.extend_from_slice(&session_id.to_le_bytes());
                            prefixed.extend_from_slice(&audio_frame);
                            if tx.send(TunnelOutgoing::Binary(prefixed)).await.is_err() {
                                break;
                            }
                        }
                    }
                }
            });

            let mut sess = sessions.lock().await;
            sess.insert(
                session_id,
                SessionState {
                    pipeline_id: sdr_id,
                    client_id,
                    forward_handle,
                },
            );

            info!(
                "Tunnel: session {} opened (client_id={}, pipeline={})",
                session_id,
                client_id,
                sess.get(&session_id).map(|s| s.pipeline_id.as_str()).unwrap_or("?")
            );
        }

        "session_text" => {
            // Client command forwarded through tunnel
            let session_id = msg.get("session_id").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            let text = msg
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let sess = sessions.lock().await;
            if let Some(session) = sess.get(&session_id) {
                let manager = state.manager.read().await;
                if let Some(pipeline) = manager.get(&session.pipeline_id) {
                    handle_tunnel_client_command(session.client_id, &text, pipeline).await;
                }
            }
        }

        "session_close" => {
            let session_id = msg.get("session_id").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            info!("Tunnel: closing session {}", session_id);

            let mut sess = sessions.lock().await;
            if let Some(session) = sess.remove(&session_id) {
                cleanup_session(state, session_id, session).await;
            }
        }

        other => {
            debug!("Tunnel: unknown message type '{}'", other);
        }
    }
}

/// Handle binary data from directory (browser → CrabSDR through tunnel).
async fn handle_directory_binary(
    _data: &[u8],
    _state: &Arc<AppState>,
    _sessions: &Arc<Mutex<HashMap<u32, SessionState>>>,
) {
    // Browser clients only send text commands (tune, set_codec), not binary.
    // Binary upstream is reserved for future use.
}

/// Handle a client command from a tunneled browser session.
async fn handle_tunnel_client_command(client_id: u64, text: &str, pipeline: &SdrPipeline) {
    let msg: serde_json::Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => {
            warn!("Tunnel client {}: invalid JSON: {}", client_id, e);
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

            let mut clients = pipeline.clients.lock().await;
            clients.update_tune(client_id, freq, mode, bandwidth);
            info!(
                "[{}] Tunnel client {} tuned to {} Hz ({})",
                pipeline.id,
                client_id,
                freq,
                mode.as_str()
            );
        }
        "set_codec" => {
            let audio = msg
                .get("audio")
                .and_then(|v| v.as_str())
                .unwrap_or("raw");
            let use_opus = audio == "opus";
            let mut clients = pipeline.clients.lock().await;
            clients.set_opus(client_id, use_opus);
        }
        _ => {
            // Tunnel clients can't use admin commands
            debug!(
                "Tunnel client {} sent unsupported command: {}",
                client_id, cmd_type
            );
        }
    }
}

/// Clean up a tunnel session.
async fn cleanup_session(state: &Arc<AppState>, session_id: u32, session: SessionState) {
    session.forward_handle.abort();
    let client_id = session.client_id;
    let manager = state.manager.read().await;
    if let Some(pipeline) = manager.get(&session.pipeline_id) {
        let mut clients = pipeline.clients.lock().await;
        clients.remove(client_id);
    }
    info!(
        "Tunnel: session {} cleaned up (client_id={})",
        session_id, client_id
    );
}
