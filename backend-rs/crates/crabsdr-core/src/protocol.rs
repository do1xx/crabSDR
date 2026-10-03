//! Binäres WebSocket-Protokoll. Jede Binärnachricht beginnt mit einem Kennbyte (Liste in docs/ARCHITECTURE.md):
//! 0x02 PCM-Ton, 0x03 JSON, 0x82 Opus, 0x84 Wasserfall-Zeile, 0x85/0x86 Wasserfall-Verlauf.

pub const TAG_AUDIO: u8 = 0x02;
pub const TAG_JSON: u8 = 0x03;
pub const TAG_AUDIO_OPUS: u8 = 0x82;
/// Wasserfall-Zeile: tag(1) + zoom(1) + start_bin(2 LE) + flags(1, Bit 0 = Delta zur vorigen Zeile) + zstd(u8-Pixel)
pub const TAG_WATERFALL: u8 = 0x84;
pub const WF_FLAG_DELTA: u8 = 0x01;
/// Wasserfall-Verlauf beim Anmelden (siehe encode_waterfall_hist)
pub const TAG_WATERFALL_HIST: u8 = 0x85;
/// Wasserfall-Verlauf als fertiges Bild: tag(1) + zoom(1) + start_bin(2 LE) + Zeilen(2 LE) + JPEG (neueste Zeile oben)
pub const TAG_WATERFALL_JPEG: u8 = 0x86;

/// Encode audio samples into binary frame.
///
/// Layout: tag(1) + i16 LE samples
pub fn encode_audio(samples: &[i16]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(1 + samples.len() * 2);
    buf.push(TAG_AUDIO);
    for &s in samples {
        buf.extend_from_slice(&s.to_le_bytes());
    }
    buf
}

/// Encode an Opus audio frame into binary frame.
///
/// Layout: tag(1) + opus_data(variable)
pub fn encode_opus_audio(opus_packet: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(1 + opus_packet.len());
    buf.push(TAG_AUDIO_OPUS);
    buf.extend_from_slice(opus_packet);
    buf
}

/// Wasserfall-Zeile kodieren (Pixelwerte u8 = 0,5-dB-Stufen ab −130 dBFS; bei `delta` Differenz zur vorigen Zeile).
pub fn encode_waterfall(zoom: u8, start_bin: u16, delta: bool, line: &[u8]) -> Vec<u8> {
    let compressed = zstd::encode_all(line, 1).unwrap_or_else(|_| line.to_vec());
    let mut buf = Vec::with_capacity(5 + compressed.len());
    buf.push(TAG_WATERFALL);
    buf.push(zoom);
    buf.extend_from_slice(&start_bin.to_le_bytes());
    buf.push(if delta { WF_FLAG_DELTA } else { 0 });
    buf.extend_from_slice(&compressed);
    buf
}

/// Wasserfall-Verlauf kodieren (älteste Zeile zuerst): erste Zeile absolut, danach Differenz zur vorigen Zeile
/// mit ±2-dB-Totzone (wie die Live-Zeilen) – rauschiger Verlauf wird so 3–4× kleiner.
/// Aufbau: tag(1) + zoom(1) + start_bin(2 LE) + Zeilen(2 LE) + flags(1, Bit 0 = Delta) + zstd(Zeilen × 1024)
pub fn encode_waterfall_hist(zoom: u8, start_bin: u16, lines: &[Vec<u8>]) -> Vec<u8> {
    let mut raw = Vec::with_capacity(lines.len() * 1024);
    let mut prev: Option<Vec<u8>> = None;
    for l in lines {
        match &mut prev {
            None => { raw.extend_from_slice(l); prev = Some(l.clone()); }
            Some(p) => {
                for (i, &a) in l.iter().enumerate() {
                    let d = a as i16 - p[i] as i16;
                    if d.abs() <= 2 { raw.push(0); } else { raw.push((d as i8) as u8); p[i] = a; }
                }
            }
        }
    }
    let compressed = zstd::encode_all(raw.as_slice(), 3).unwrap_or(raw);
    let mut buf = Vec::with_capacity(7 + compressed.len());
    buf.push(TAG_WATERFALL_HIST);
    buf.push(zoom);
    buf.extend_from_slice(&start_bin.to_le_bytes());
    buf.extend_from_slice(&(lines.len() as u16).to_le_bytes());
    buf.push(WF_FLAG_DELTA);
    buf.extend_from_slice(&compressed);
    buf
}

/// Wasserfall-Zeile dekodieren → (zoom, start_bin, delta, Pixel).
pub fn decode_waterfall(frame: &[u8]) -> Option<(u8, u16, bool, Vec<u8>)> {
    if frame.len() < 5 || frame[0] != TAG_WATERFALL {
        return None;
    }
    let start = u16::from_le_bytes([frame[2], frame[3]]);
    let line = zstd::decode_all(&frame[5..]).ok()?;
    Some((frame[1], start, frame[4] & WF_FLAG_DELTA != 0, line))
}

/// Encode a JSON control message.
pub fn encode_json(json: &str) -> Vec<u8> {
    let mut buf = Vec::with_capacity(1 + json.len());
    buf.push(TAG_JSON);
    buf.extend_from_slice(json.as_bytes());
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_waterfall_roundtrip() {
        let line: Vec<u8> = (0..1024u32).map(|i| (i % 251) as u8).collect();
        let f = encode_waterfall(1, 512, true, &line);
        assert!(f.len() < 1024, "zstd sollte komprimieren: {}", f.len());
        let (z, s, d, l) = decode_waterfall(&f).unwrap();
        assert_eq!((z, s, d), (1, 512, true));
        assert_eq!(l, line);
    }

    #[test]
    fn test_audio_encode() {
        let samples = vec![0i16, 1000, -1000, 32767];
        let frame = encode_audio(&samples);
        assert_eq!(frame[0], TAG_AUDIO);
        assert_eq!(frame.len(), 1 + 8);
    }
}
