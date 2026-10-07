use thiserror::Error;

#[derive(Debug, Error)]
pub enum CrabSdrError {
    #[error("SDR error: {0}")]
    Sdr(String),

    #[error("SDR process exited with code {0}")]
    SdrProcessDied(i32),

    #[error("SDR not connected")]
    SdrNotConnected,

    #[error("DSP error: {0}")]
    Dsp(String),

    #[error("Config error: {0}")]
    Config(String),

    #[error("Plugin error: {0}")]
    Plugin(String),

    #[error("WebSocket error: {0}")]
    WebSocket(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}
