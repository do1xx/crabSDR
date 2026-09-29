//! rtl_tcp network SDR driver.
//!
//! Connects to rtl_tcp server over TCP, reads u8 IQ stream,
//! sends commands (frequency, gain, etc.) via 5-byte protocol.
//!
//! Port of backend/sdr/manager.py (rtl_tcp path)

use crate::{u8_to_complex, DriverCommand, SdrConfig};
use crabsdr_core::IqBuffer;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, watch};
use tracing::{debug, info, warn};

// rtl_tcp command codes
const CMD_SET_FREQUENCY: u8 = 0x01;
const CMD_SET_SAMPLE_RATE: u8 = 0x02;
const CMD_SET_GAIN_MODE: u8 = 0x03;
const CMD_SET_GAIN: u8 = 0x04;
const CMD_SET_FREQ_CORRECTION: u8 = 0x05;
const CMD_SET_BIAS_TEE: u8 = 0x0e;
/// Stall-Wächter: so lange ohne ein Byte gilt die Quelle als tot (ein Verteiler vor rtl_tcp kann bis 10 s brauchen).
const STALL_TIMEOUT_S: u64 = 5;

pub struct RtlTcpDriver {
    config: SdrConfig,
}

impl RtlTcpDriver {
    pub fn new(config: SdrConfig) -> Self {
        Self { config }
    }

    pub async fn run(
        self,
        iq_tx: mpsc::Sender<IqBuffer>,
        mut cmd_rx: mpsc::Receiver<DriverCommand>,
        health_tx: watch::Sender<bool>,
    ) {
        // Fehlt die Quelle dauerhaft (Stick nicht gesteckt), kreist der Treiber: erster Fehlschlag und danach höchstens
        // alle 10 min eine Zeile, Abstand 2 → 30 s. Nach einer erfolgreichen Verbindung beginnt die Zählung neu.
        let addr = format!("{}:{}", self.config.tcp_host, self.config.tcp_port);
        let mut fails = 0u32;
        let mut down_since = std::time::Instant::now();
        let mut last_note = down_since;

        loop {
            match self.connect_and_stream(&iq_tx, &mut cmd_rx, &health_tx).await {
                TcpResult::Stopped => {
                    let _ = health_tx.send(false);
                    info!("rtl_tcp stopped");
                    break;
                }
                TcpResult::Disconnected => {
                    let was_up = *health_tx.borrow();
                    let _ = health_tx.send(false);
                    let now = std::time::Instant::now();
                    if was_up {
                        fails = 0;
                    }
                    if fails == 0 {
                        down_since = now;
                        last_note = now;
                        if was_up {
                            warn!("rtl_tcp {}: Verbindung verloren, verbinde neu", addr);
                        } else {
                            warn!("rtl_tcp {}: keine Verbindung, neue Versuche (Meldung höchstens alle 10 min)", addr);
                        }
                    } else if now.duration_since(last_note).as_secs() >= 600 {
                        last_note = now;
                        warn!("rtl_tcp {}: seit {} min keine Verbindung ({} Versuche)", addr, now.duration_since(down_since).as_secs() / 60, fails);
                    }
                    fails += 1;
                    let delay = 2u64.pow(fails.min(5)).min(30);
                    tokio::time::sleep(tokio::time::Duration::from_secs(delay)).await;
                }
            }
        }
    }

    async fn connect_and_stream(
        &self,
        iq_tx: &mpsc::Sender<IqBuffer>,
        cmd_rx: &mut mpsc::Receiver<DriverCommand>,
        health_tx: &watch::Sender<bool>,
    ) -> TcpResult {
        let addr = format!("{}:{}", self.config.tcp_host, self.config.tcp_port);
        debug!("Connecting to rtl_tcp at {}...", addr);

        let mut stream = match tokio::time::timeout(
            tokio::time::Duration::from_secs(10),
            TcpStream::connect(&addr),
        )
        .await
        {
            Ok(Ok(stream)) => stream,
            Ok(Err(e)) => {
                debug!("rtl_tcp connection failed: {}", e);
                return TcpResult::Disconnected;
            }
            Err(_) => {
                debug!("rtl_tcp connection timed out");
                return TcpResult::Disconnected;
            }
        };

        // Read 12-byte header: "RTL0" + tuner_type(u32) + gain_count(u32)
        let mut header = [0u8; 12];
        match tokio::time::timeout(
            tokio::time::Duration::from_secs(5),
            stream.read_exact(&mut header),
        )
        .await
        {
            Ok(Ok(_)) => {
                let magic = &header[0..4];
                info!("rtl_tcp connected (magic: {:?})", std::str::from_utf8(magic).unwrap_or("???"));
            }
            _ => {
                debug!("rtl_tcp header read failed");
                return TcpResult::Disconnected;
            }
        }

        // Send initial configuration
        let c = &self.config;
        if send_cmd(&mut stream, CMD_SET_FREQUENCY, c.center_freq as u32).await.is_err()
            || send_cmd(&mut stream, CMD_SET_SAMPLE_RATE, c.sample_rate).await.is_err()
            || send_cmd(&mut stream, CMD_SET_GAIN_MODE, 1).await.is_err()
            || send_cmd(&mut stream, CMD_SET_GAIN, (c.gain * 10.0) as u32).await.is_err()
        {
            return TcpResult::Disconnected;
        }
        if c.ppm != 0 {
            let _ = send_cmd(&mut stream, CMD_SET_FREQ_CORRECTION, c.ppm as u32).await;
        }
        if c.bias_tee {
            let _ = send_cmd(&mut stream, CMD_SET_BIAS_TEE, 1).await;
        }

        info!(
            "rtl_tcp configured: {} Hz, {} sps, gain {}",
            c.center_freq, c.sample_rate, c.gain
        );
        let _ = health_tx.send(true);

        // Split stream for concurrent read/write
        let (mut reader, mut writer) = stream.into_split();

        let chunk_size = 8192;
        let mut buf = vec![0u8; chunk_size];

        loop {
            tokio::select! {
                result = tokio::time::timeout(tokio::time::Duration::from_secs(STALL_TIMEOUT_S), reader.read_exact(&mut buf)) => {
                    match result {
                        Err(_) => {
                            // Stall: Verbindung offen, aber keine Daten (eingefrorener Stick, toter Mux) -> neu verbinden
                            warn!("rtl_tcp: {} s keine Daten von {} — Verbindung wird neu aufgebaut", STALL_TIMEOUT_S, addr);
                            return TcpResult::Disconnected;
                        }
                        Ok(Ok(_)) => {
                            let iq = u8_to_complex(&buf);
                            if iq_tx.send(iq).await.is_err() {
                                return TcpResult::Stopped;
                            }
                        }
                        Ok(Err(_)) => return TcpResult::Disconnected,
                    }
                }

                cmd = cmd_rx.recv() => {
                    match cmd {
                        Some(DriverCommand::Stop) | None => return TcpResult::Stopped,
                        Some(DriverCommand::SetFrequency(freq)) => {
                            if send_cmd_writer(&mut writer, CMD_SET_FREQUENCY, freq as u32).await.is_err() {
                                return TcpResult::Disconnected;
                            }
                            info!("rtl_tcp frequency set to {} Hz", freq);
                        }
                        Some(DriverCommand::SetGain(gain)) => {
                            if send_cmd_writer(&mut writer, CMD_SET_GAIN, (gain * 10.0) as u32).await.is_err() {
                                return TcpResult::Disconnected;
                            }
                            info!("rtl_tcp gain set to {}", gain);
                        }
                        Some(DriverCommand::SetBiasTee(enabled)) => {
                            if send_cmd_writer(&mut writer, CMD_SET_BIAS_TEE, if enabled { 1 } else { 0 }).await.is_err() {
                                return TcpResult::Disconnected;
                            }
                            info!("rtl_tcp bias_tee set to {}", enabled);
                        }
                        Some(DriverCommand::SetPpm(ppm)) => {
                            if send_cmd_writer(&mut writer, CMD_SET_FREQ_CORRECTION, ppm as u32).await.is_err() {
                                return TcpResult::Disconnected;
                            }
                            info!("rtl_tcp PPM set to {}", ppm);
                        }
                        Some(DriverCommand::SetSampleRate(rate)) => {
                            if send_cmd_writer(&mut writer, CMD_SET_SAMPLE_RATE, rate).await.is_err() {
                                return TcpResult::Disconnected;
                            }
                            info!("rtl_tcp sample_rate set to {}", rate);
                        }
                        Some(DriverCommand::SetGainElements(_)) => {
                            // rtl_tcp doesn't support per-element gains, ignore
                        }
                    }
                }
            }
        }
    }
}

/// Send a 5-byte rtl_tcp command: 1 byte cmd + 4 bytes param (big-endian).
async fn send_cmd(stream: &mut TcpStream, cmd: u8, param: u32) -> Result<(), std::io::Error> {
    let mut buf = [0u8; 5];
    buf[0] = cmd;
    buf[1..5].copy_from_slice(&param.to_be_bytes());
    stream.write_all(&buf).await
}

async fn send_cmd_writer(
    writer: &mut tokio::net::tcp::OwnedWriteHalf,
    cmd: u8,
    param: u32,
) -> Result<(), std::io::Error> {
    let mut buf = [0u8; 5];
    buf[0] = cmd;
    buf[1..5].copy_from_slice(&param.to_be_bytes());
    writer.write_all(&buf).await
}

enum TcpResult {
    Stopped,
    Disconnected,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    /// Quelle schickt den Header und dann nichts mehr → nach STALL_TIMEOUT_S muss der Treiber „offline“ melden.
    #[tokio::test]
    async fn stall_is_detected_and_reconnected() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let (mut s, _) = listener.accept().await.unwrap();
                let mut hdr = [0u8; 12];
                hdr[..4].copy_from_slice(b"RTL0");
                let _ = s.write_all(&hdr).await;
                // Befehle lesen, aber keine Daten liefern
                let mut buf = [0u8; 64];
                while s.read(&mut buf).await.unwrap_or(0) > 0 {}
            }
        });
        let cfg = SdrConfig {
            driver: "rtl_tcp".into(), device: "0".into(), tcp_host: "127.0.0.1".into(), tcp_port: port,
            center_freq: 145_000_000, sample_rate: 2_048_000, gain: 10.0, ppm: 0, bias_tee: false,
            gain_elements: Default::default(),
        format: None, };
        let (iq_tx, _iq_rx) = mpsc::channel(4);
        let (cmd_tx, cmd_rx) = mpsc::channel(4);
        let (health_tx, mut health_rx) = watch::channel(false);
        let join = tokio::spawn(RtlTcpDriver::new(cfg).run(iq_tx, cmd_rx, health_tx));
        // online …
        tokio::time::timeout(std::time::Duration::from_secs(3), async { while !*health_rx.borrow() { health_rx.changed().await.unwrap(); } })
            .await.expect("Quelle sollte online gehen");
        // … und nach dem Stall wieder offline (Timeout 5 s + Reserve)
        tokio::time::timeout(std::time::Duration::from_secs(STALL_TIMEOUT_S + 3), async { while *health_rx.borrow() { health_rx.changed().await.unwrap(); } })
            .await.expect("Stall wurde nicht erkannt");
        let _ = cmd_tx.send(DriverCommand::Stop).await;
        join.abort();
    }
}
