//! Minimal WAV support: write 16-bit mono PCM, read 8/16/24/32-bit PCM or 32-bit float.

use std::io::{Read, Write};
use std::path::Path;

use anyhow::{Context, bail};

pub fn write_pcm16_mono(path: &Path, samples: &[f32], sample_rate: u32) -> anyhow::Result<()> {
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    let data_len = (samples.len() * 2) as u32;
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        let v = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    let mut f = std::fs::File::create(path).with_context(|| format!("creating {}", path.display()))?;
    f.write_all(&out)?;
    Ok(())
}

/// Reads a WAV file and returns (mono samples averaged across channels, sample rate).
pub fn read_mono(path: &Path) -> anyhow::Result<(Vec<f32>, u32)> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .with_context(|| format!("opening {}", path.display()))?
        .read_to_end(&mut bytes)?;
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        bail!("not a RIFF/WAVE file");
    }
    let (mut format, mut channels, mut rate, mut bits) = (0u16, 0u16, 0u32, 0u16);
    let mut pos = 12;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let len = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body = &bytes[pos + 8..(pos + 8 + len).min(bytes.len())];
        if id == b"fmt " && body.len() >= 16 {
            format = u16::from_le_bytes([body[0], body[1]]);
            channels = u16::from_le_bytes([body[2], body[3]]);
            rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
            bits = u16::from_le_bytes([body[14], body[15]]);
            if format == 0xFFFE && body.len() >= 26 {
                format = u16::from_le_bytes([body[24], body[25]]); // WAVE_FORMAT_EXTENSIBLE subformat
            }
        } else if id == b"data" {
            if channels == 0 {
                bail!("data chunk before fmt chunk");
            }
            let frame: Vec<f32> = match (format, bits) {
                (1, 8) => body.iter().map(|&b| (b as f32 - 128.0) / 128.0).collect(),
                (1, 16) => body
                    .chunks_exact(2)
                    .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
                    .collect(),
                (1, 24) => body
                    .chunks_exact(3)
                    .map(|c| (i32::from_le_bytes([0, c[0], c[1], c[2]]) >> 8) as f32 / 8_388_608.0)
                    .collect(),
                (1, 32) => body
                    .chunks_exact(4)
                    .map(|c| i32::from_le_bytes(c.try_into().unwrap()) as f32 / 2_147_483_648.0)
                    .collect(),
                (3, 32) => body.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect(),
                _ => bail!("unsupported WAV encoding (format {format}, {bits} bits)"),
            };
            let ch = channels as usize;
            let mono = frame.chunks_exact(ch).map(|f| f.iter().sum::<f32>() / ch as f32).collect();
            return Ok((mono, rate));
        }
        pos += 8 + len + (len & 1);
    }
    bail!("no data chunk found")
}
