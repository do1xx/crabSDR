/// Binary WebSocket protocol.
///
/// All binary messages start with a 1-byte tag:
///   0x01 — Spectrum: center_freq(u64 LE) + sample_rate(u32 LE) + n_bins(u16 LE) + i8[] bins
///   0x02 — Audio: i16 LE samples
///   0x03 — JSON control message (UTF-8 bytes after tag)
///   0x81 — Spectrum zstd-compressed (same payload as 0x01, but zstd-wrapped)

pub const TAG_SPECTRUM: u8 = 0x01;
pub const TAG_AUDIO: u8 = 0x02;
pub const TAG_JSON: u8 = 0x03;
pub const TAG_SPECTRUM_ZSTD: u8 = 0x81;
pub const TAG_AUDIO_OPUS: u8 = 0x82;
/// Wasserfall-Zeile: tag(1) + zoom(1) + start_bin(2 LE) + flags(1, Bit 0 = Delta zur vorigen Zeile) + zstd(u8-Pixel)
pub const TAG_WATERFALL: u8 = 0x84;
pub const WF_FLAG_DELTA: u8 = 0x01;
/// Wasserfall-Verlauf beim Anmelden (siehe encode_waterfall_hist)
pub const TAG_WATERFALL_HIST: u8 = 0x85;
/// Wasserfall-Verlauf als fertiges Bild: tag(1) + zoom(1) + start_bin(2 LE) + Zeilen(2 LE) + JPEG (neueste Zeile oben)
pub const TAG_WATERFALL_JPEG: u8 = 0x86;

/// Encode spectrum data into binary frame.
///
/// Layout: tag(1) + center_freq(8) + sample_rate(4) + n_bins(2) + bins(n_bins)
pub fn encode_spectrum(center_freq: u64, sample_rate: u32, bins: &[i8]) -> Vec<u8> {
    let n_bins = bins.len() as u16;
    let mut buf = Vec::with_capacity(1 + 8 + 4 + 2 + bins.len());
    buf.push(TAG_SPECTRUM);
    buf.extend_from_slice(&center_freq.to_le_bytes());
    buf.extend_from_slice(&sample_rate.to_le_bytes());
    buf.extend_from_slice(&n_bins.to_le_bytes());
    // Safety: i8 and u8 have same layout
    buf.extend_from_slice(bytemuck_i8_to_u8(bins));
    buf
}

/// Encode spectrum with zstd compression.
pub fn encode_spectrum_zstd(center_freq: u64, sample_rate: u32, bins: &[i8]) -> Vec<u8> {
    let payload = encode_spectrum_payload(center_freq, sample_rate, bins);
    let compressed = zstd::encode_all(payload.as_slice(), 1).unwrap_or(payload);
    let mut buf = Vec::with_capacity(1 + compressed.len());
    buf.push(TAG_SPECTRUM_ZSTD);
    buf.extend_from_slice(&compressed);
    buf
}

fn encode_spectrum_payload(center_freq: u64, sample_rate: u32, bins: &[i8]) -> Vec<u8> {
    let n_bins = bins.len() as u16;
    let mut buf = Vec::with_capacity(8 + 4 + 2 + bins.len());
    buf.extend_from_slice(&center_freq.to_le_bytes());
    buf.extend_from_slice(&sample_rate.to_le_bytes());
    buf.extend_from_slice(&n_bins.to_le_bytes());
    buf.extend_from_slice(bytemuck_i8_to_u8(bins));
    buf
}

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

fn bytemuck_i8_to_u8(slice: &[i8]) -> &[u8] {
    // Safety: i8 and u8 have identical size/alignment
    unsafe { std::slice::from_raw_parts(slice.as_ptr() as *const u8, slice.len()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spectrum_encode() {
        let bins = vec![-120i8, -80, -60, -40];
        let frame = encode_spectrum(100_000_000, 2_048_000, &bins);
        assert_eq!(frame[0], TAG_SPECTRUM);
        let freq = u64::from_le_bytes(frame[1..9].try_into().unwrap());
        assert_eq!(freq, 100_000_000);
        let rate = u32::from_le_bytes(frame[9..13].try_into().unwrap());
        assert_eq!(rate, 2_048_000);
        let n = u16::from_le_bytes(frame[13..15].try_into().unwrap());
        assert_eq!(n, 4);
    }

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
