//! Framed request/response protocol between the daemon and its helper processes over
//! the helpers' stdin/stdout pipes.
//!
//! Frame layout: `u32 little-endian length` (of what follows) + `u8 kind` + payload.
//! JSON frames carry control messages; binary frames carry raw f32 PCM audio.

use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize, de::DeserializeOwned};

pub const KIND_JSON: u8 = 1;
pub const KIND_BINARY: u8 = 2;

/// Refuse absurd frames (corrupt stream) instead of allocating gigabytes.
const MAX_FRAME: usize = 256 * 1024 * 1024;

pub fn write_frame<W: Write>(w: &mut W, kind: u8, payload: &[u8]) -> io::Result<()> {
    let len = u32::try_from(payload.len() + 1)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "frame too large"))?;
    w.write_all(&len.to_le_bytes())?;
    w.write_all(&[kind])?;
    w.write_all(payload)?;
    w.flush()
}

pub fn read_frame<R: Read>(r: &mut R) -> io::Result<(u8, Vec<u8>)> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len) as usize;
    if len == 0 || len > MAX_FRAME {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("bad frame length {len}")));
    }
    let mut kind = [0u8; 1];
    r.read_exact(&mut kind)?;
    let mut payload = vec![0u8; len - 1];
    r.read_exact(&mut payload)?;
    Ok((kind[0], payload))
}

pub fn send_json<W: Write, T: Serialize>(w: &mut W, msg: &T) -> io::Result<()> {
    let bytes = serde_json::to_vec(msg).map_err(io::Error::other)?;
    write_frame(w, KIND_JSON, &bytes)
}

pub fn parse_json<T: DeserializeOwned>(payload: &[u8]) -> io::Result<T> {
    serde_json::from_slice(payload).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

pub fn recv_json<R: Read, T: DeserializeOwned>(r: &mut R) -> io::Result<T> {
    let (kind, payload) = read_frame(r)?;
    if kind != KIND_JSON {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "expected a JSON frame"));
    }
    parse_json(&payload)
}

pub fn send_samples<W: Write>(w: &mut W, samples: &[f32]) -> io::Result<()> {
    let mut bytes = Vec::with_capacity(samples.len() * 4);
    for s in samples {
        bytes.extend_from_slice(&s.to_le_bytes());
    }
    write_frame(w, KIND_BINARY, &bytes)
}

pub fn recv_samples<R: Read>(r: &mut R) -> io::Result<Vec<f32>> {
    let (kind, payload) = read_frame(r)?;
    if kind != KIND_BINARY || payload.len() % 4 != 0 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "expected a PCM frame"));
    }
    Ok(payload
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect())
}

// ---------------------------------------------------------------------------------------------
// Speech-to-text helper (audian-stt)

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SttRequest {
    /// Load (or switch to) a model. Sent as soon as recording starts so loading overlaps speech.
    Load { model_path: String, threads: u32 },
    /// Followed immediately by one binary frame of 16 kHz mono f32 samples.
    /// With `language == "auto"`, a non-empty `candidates` list restricts detection to those
    /// languages (e.g. ["en", "pt"] for a bilingual speaker).
    Transcribe {
        id: u64,
        language: String,
        #[serde(default)]
        candidates: Vec<String>,
        initial_prompt: String,
    },
    Shutdown,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SttResponse {
    Loaded { model: String, load_ms: u64 },
    Transcript { id: u64, text: String, language: String, elapsed_ms: u64, no_speech_prob: f32 },
    Error { id: Option<u64>, message: String },
}

// ---------------------------------------------------------------------------------------------
// Local LLM helper (audian-llm)

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LlmRequest {
    Load { model_path: String, threads: u32, context_tokens: u32 },
    /// Pre-computes the prompt prefix (system prompt + examples) so a later `Generate`
    /// with the same messages only has to process the transcript.
    Prepare { prefix: Vec<ChatMessage> },
    Generate { id: u64, prefix: Vec<ChatMessage>, user: String, max_tokens: u32 },
    Shutdown,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn new(role: &str, content: impl Into<String>) -> Self {
        Self { role: role.into(), content: content.into() }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LlmResponse {
    Loaded { model: String, load_ms: u64 },
    Prepared { prefix_tokens: u32, elapsed_ms: u64 },
    Output { id: u64, text: String, prompt_tokens: u32, generated_tokens: u32, elapsed_ms: u64 },
    Error { id: Option<u64>, message: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_roundtrip() {
        let mut buf = Vec::new();
        send_json(&mut buf, &SttRequest::Shutdown).unwrap();
        send_samples(&mut buf, &[0.5, -0.25]).unwrap();
        let mut r = buf.as_slice();
        assert!(matches!(recv_json::<_, SttRequest>(&mut r).unwrap(), SttRequest::Shutdown));
        assert_eq!(recv_samples(&mut r).unwrap(), vec![0.5, -0.25]);
    }
}
