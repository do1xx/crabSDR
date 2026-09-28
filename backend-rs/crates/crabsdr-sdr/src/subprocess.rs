//! Subprocess-based SDR driver.
//!
//! Starts rtl_sdr / rx_sdr / hackrf_transfer as a child process,
//! reads raw u8 IQ from stdout, converts to Complex32, sends via mpsc.
//!
//! Port of backend/sdr/manager.py (subprocess path)

use crate::{u8_to_complex, DriverCommand, SdrConfig};
use crabsdr_core::IqBuffer;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, watch};
use tracing::{error, info, warn};

pub struct SubprocessDriver {
    config: SdrConfig,
}

impl SubprocessDriver {
    pub fn new(config: SdrConfig) -> Self {
        Self { config }
    }

    /// Maximum consecutive failures before giving up (avoids infinite restart loop).
    const MAX_CONSECUTIVE_FAILURES: u32 = 10;

    pub async fn run(
        mut self,
        iq_tx: mpsc::Sender<IqBuffer>,
        mut cmd_rx: mpsc::Receiver<DriverCommand>,
        health_tx: watch::Sender<bool>,
    ) {
        let mut restart_count = 0u32;
        let sdr_label = format!("{}:{}", self.config.driver, self.config.device);

        loop {
            // Give up after too many consecutive failures
            if restart_count >= Self::MAX_CONSECUTIVE_FAILURES {
                error!(
                    "[{}] SDR failed {} times in a row — giving up. Device is offline.",
                    sdr_label, restart_count
                );
                let _ = health_tx.send(false);
                // Park the task and wait for a reconfigure command to try again
                loop {
                    match cmd_rx.recv().await {
                        Some(DriverCommand::SetFrequency(_))
                        | Some(DriverCommand::SetGain(_))
                        | Some(DriverCommand::SetGainElements(_))
                        | Some(DriverCommand::SetBiasTee(_))
                        | Some(DriverCommand::SetPpm(_))
                        | Some(DriverCommand::SetSampleRate(_)) => {
                            info!("[{}] Received reconfigure command — retrying SDR", sdr_label);
                            restart_count = 0;
                            break;
                        }
                        Some(DriverCommand::Stop) | None => return,
                    }
                }
            }

            let cmd = self.build_command();
            info!("[{}] Starting SDR: {}", sdr_label, cmd.join(" "));

            match self.run_process(&cmd, &iq_tx, &mut cmd_rx, &health_tx, &sdr_label).await {
                ProcessResult::Stopped => {
                    let _ = health_tx.send(false);
                    info!("[{}] SDR stopped by command", sdr_label);
                    break;
                }
                ProcessResult::Restarted(new_config) => {
                    self.config = new_config;
                    restart_count = 0;
                    // Give USB stack time to release the device before reopening
                    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                    continue;
                }
                ProcessResult::Died { code, ran_ok } => {
                    let _ = health_tx.send(false);
                    if ran_ok {
                        // Process ran successfully — reset backoff
                        restart_count = 0;
                        let delay = 2;
                        warn!(
                            "[{}] SDR died (code {:?}) after successful run, restart in {}s",
                            sdr_label, code, delay
                        );
                        tokio::time::sleep(tokio::time::Duration::from_secs(delay)).await;
                    } else {
                        restart_count += 1;
                        let delay = (2u64.pow(restart_count.min(4))).min(15);
                        warn!(
                            "[{}] SDR died immediately (code {:?}), restart #{}/{} in {}s",
                            sdr_label, code, restart_count, Self::MAX_CONSECUTIVE_FAILURES, delay
                        );
                        tokio::time::sleep(tokio::time::Duration::from_secs(delay)).await;
                    }
                }
                ProcessResult::SpawnFailed => {
                    let _ = health_tx.send(false);
                    restart_count += 1;
                    let delay = (2u64.pow(restart_count.min(4))).min(15);
                    error!("[{}] SDR spawn failed, retry #{}/{} in {}s",
                        sdr_label, restart_count, Self::MAX_CONSECUTIVE_FAILURES, delay);
                    tokio::time::sleep(tokio::time::Duration::from_secs(delay)).await;
                }
            }
        }
    }

    async fn run_process(
        &self,
        cmd: &[String],
        iq_tx: &mpsc::Sender<IqBuffer>,
        cmd_rx: &mut mpsc::Receiver<DriverCommand>,
        health_tx: &watch::Sender<bool>,
        sdr_label: &str,
    ) -> ProcessResult {
        let mut child = match Command::new(&cmd[0])
            .args(&cmd[1..])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
        {
            Ok(child) => {
                info!("[{}] SDR process started (PID {:?})", sdr_label, child.id());
                child
            }
            Err(e) => {
                error!("[{}] Failed to start SDR: {}", sdr_label, e);
                return ProcessResult::SpawnFailed;
            }
        };

        let mut stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();

        // Spawn a task to log stderr lines from the subprocess
        let stderr_label = sdr_label.to_string();
        let stderr_task = tokio::spawn(async move {
            let reader = BufReader::new(stderr);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if !line.is_empty() {
                    warn!("[{}] stderr: {}", stderr_label, line);
                }
            }
        });

        // Read IQ in chunks: fft_size * 2 bytes = fft_size IQ samples
        let chunk_size = 8192;
        let mut buf = vec![0u8; chunk_size];
        let mut ran_ok = false;
        let mut iq_send_count: u64 = 0;

        let mut last_iq_time = tokio::time::Instant::now();

        loop {
            tokio::select! {
                // Read IQ data from subprocess stdout
                result = stdout.read_exact(&mut buf) => {
                    match result {
                        Ok(_) => {
                            last_iq_time = tokio::time::Instant::now();
                            if !ran_ok {
                                let _ = health_tx.send(true);
                                ran_ok = true;
                            }
                            iq_send_count += 1;
                            if iq_send_count <= 5 || iq_send_count % 5000 == 0 {
                                info!("[{}] IQ chunk #{} ({} bytes)", sdr_label, iq_send_count, buf.len());
                            }
                            let iq = u8_to_complex(&buf);
                            // Use try_send to avoid blocking if DSP thread is slow
                            match iq_tx.try_send(iq) {
                                Ok(()) => {}
                                Err(mpsc::error::TrySendError::Full(iq)) => {
                                    if iq_send_count <= 5 || iq_send_count % 1000 == 0 {
                                        warn!("[{}] IQ channel full (cap {}), awaiting send...", sdr_label, iq_tx.capacity());
                                    }
                                    // Fall back to blocking send
                                    if iq_tx.send(iq).await.is_err() {
                                        let _ = child.kill().await;
                                        stderr_task.abort();
                                        return ProcessResult::Stopped;
                                    }
                                }
                                Err(mpsc::error::TrySendError::Closed(_)) => {
                                    // Receiver dropped — shut down
                                    let _ = child.kill().await;
                                    stderr_task.abort();
                                    return ProcessResult::Stopped;
                                }
                            }
                        }
                        Err(_) => {
                            // stdout closed — process died, wait for stderr to finish
                            let status = child.wait().await.ok();
                            let code = status.and_then(|s| s.code());
                            let _ = tokio::time::timeout(
                                tokio::time::Duration::from_secs(1),
                                stderr_task,
                            ).await;
                            return ProcessResult::Died { code, ran_ok };
                        }
                    }
                }

                // Watchdog: restart if no IQ data for 10 seconds
                _ = tokio::time::sleep(tokio::time::Duration::from_secs(10)) => {
                    if last_iq_time.elapsed() > tokio::time::Duration::from_secs(10) {
                        warn!("[{}] No IQ data for {}s — subprocess stalled, restarting",
                            sdr_label, last_iq_time.elapsed().as_secs());
                        let _ = child.kill().await;
                        let _ = child.wait().await;
                        stderr_task.abort();
                        return ProcessResult::Died { code: None, ran_ok };
                    }
                }

                // Handle commands
                cmd = cmd_rx.recv() => {
                    match cmd {
                        Some(DriverCommand::Stop) | None => {
                            let _ = child.kill().await;
                            stderr_task.abort();
                            return ProcessResult::Stopped;
                        }
                        Some(DriverCommand::SetFrequency(mut freq)) => {
                            // Drain any queued SetFrequency commands, use only the last one
                            while let Ok(cmd) = cmd_rx.try_recv() {
                                match cmd {
                                    DriverCommand::SetFrequency(f) => freq = f,
                                    DriverCommand::Stop => {
                                        let _ = child.kill().await;
                                        stderr_task.abort();
                                        return ProcessResult::Stopped;
                                    }
                                    _ => {}
                                }
                            }
                            // Only restart if frequency actually changed
                            if freq != self.config.center_freq {
                                let _ = child.kill().await;
                                let _ = child.wait().await;
                                stderr_task.abort();
                                let mut new_config = self.config.clone();
                                new_config.center_freq = freq;
                                info!("Retuning SDR to {} Hz", freq);
                                return ProcessResult::Restarted(new_config);
                            }
                        }
                        Some(DriverCommand::SetGain(gain)) => {
                            let _ = child.kill().await;
                            let _ = child.wait().await;
                            stderr_task.abort();
                            let mut new_config = self.config.clone();
                            new_config.gain = gain;
                            new_config.gain_elements.clear();
                            return ProcessResult::Restarted(new_config);
                        }
                        Some(DriverCommand::SetGainElements(elements)) => {
                            let _ = child.kill().await;
                            let _ = child.wait().await;
                            stderr_task.abort();
                            let mut new_config = self.config.clone();
                            new_config.gain_elements = elements;
                            info!("Restarting SDR with per-element gains: {:?}", new_config.gain_elements);
                            return ProcessResult::Restarted(new_config);
                        }
                        Some(DriverCommand::SetBiasTee(enabled)) => {
                            let _ = child.kill().await;
                            let _ = child.wait().await;
                            stderr_task.abort();
                            let mut new_config = self.config.clone();
                            new_config.bias_tee = enabled;
                            info!("Restarting SDR with bias_tee={}", enabled);
                            return ProcessResult::Restarted(new_config);
                        }
                        Some(DriverCommand::SetPpm(ppm)) => {
                            let _ = child.kill().await;
                            let _ = child.wait().await;
                            stderr_task.abort();
                            let mut new_config = self.config.clone();
                            new_config.ppm = ppm;
                            info!("Restarting SDR with ppm={}", ppm);
                            return ProcessResult::Restarted(new_config);
                        }
                        Some(DriverCommand::SetSampleRate(rate)) => {
                            let _ = child.kill().await;
                            let _ = child.wait().await;
                            stderr_task.abort();
                            let mut new_config = self.config.clone();
                            new_config.sample_rate = rate;
                            info!("Restarting SDR with sample_rate={}", rate);
                            return ProcessResult::Restarted(new_config);
                        }
                    }
                }
            }
        }
    }

    fn build_command(&self) -> Vec<String> {
        let c = &self.config;
        match c.driver.as_str() {
            "rtl_sdr" => {
                // Use per-element TUNER gain if set, otherwise overall gain
                let gain_val = c.gain_elements.get("TUNER").copied().unwrap_or(c.gain);
                let mut cmd = vec![
                    "rtl_sdr".into(),
                    "-f".into(), c.center_freq.to_string(),
                    "-s".into(), c.sample_rate.to_string(),
                    "-g".into(), gain_val.to_string(),
                    "-d".into(), c.device.clone(),
                ];
                if c.ppm != 0 {
                    cmd.push("-p".into());
                    cmd.push(c.ppm.to_string());
                }
                if c.bias_tee {
                    cmd.push("-T".into());
                }
                cmd.push("-".into());
                cmd
            }
            "hackrf" => {
                let mut cmd = vec![
                    "hackrf_transfer".into(),
                    "-r".into(), "/dev/stdout".into(),
                    "-f".into(), c.center_freq.to_string(),
                    "-s".into(), c.sample_rate.to_string(),
                ];
                // HackRF per-element gains: LNA (-l), VGA (-g), AMP (-a)
                if c.gain_elements.is_empty() {
                    cmd.push("-g".into());
                    cmd.push(c.gain.to_string());
                } else {
                    if let Some(lna) = c.gain_elements.get("LNA") {
                        cmd.push("-l".into());
                        cmd.push((*lna as u32).to_string());
                    }
                    if let Some(vga) = c.gain_elements.get("VGA") {
                        cmd.push("-g".into());
                        cmd.push((*vga as u32).to_string());
                    }
                    if let Some(amp) = c.gain_elements.get("AMP") {
                        if *amp > 0.0 {
                            cmd.push("-a".into());
                            cmd.push("1".into());
                        }
                    }
                }
                cmd
            }
            "rx_sdr" | "soapy" => {
                let gain_str = if c.gain_elements.is_empty() {
                    c.gain.to_string()
                } else {
                    c.gain_elements.iter()
                        .map(|(k, v)| format!("{}={}", k, v))
                        .collect::<Vec<_>>()
                        .join(",")
                };
                vec![
                    "rx_sdr".into(),
                    "-f".into(), c.center_freq.to_string(),
                    "-s".into(), c.sample_rate.to_string(),
                    "-g".into(), gain_str,
                    "-d".into(), c.device.clone(),
                    "-".into(),
                ]
            }
            other => {
                error!("Unknown SDR driver: {}", other);
                vec!["false".into()] // Will fail immediately
            }
        }
    }
}

enum ProcessResult {
    Stopped,
    Restarted(SdrConfig),
    /// Process died. `ran_ok` = true if it delivered IQ data before dying.
    Died { code: Option<i32>, ran_ok: bool },
    SpawnFailed,
}
