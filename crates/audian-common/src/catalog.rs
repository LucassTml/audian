//! Curated models Audian can download, with the resource figures shown in the model manager.
//!
//! RAM figures are the helper process's working set while the model is loaded and in use,
//! including the memory-mapped model file (measured on an i9-12900KS for the recommended
//! models; others estimated from model size and KV-cache shape). Engines exit after use by
//! default, so this is only held during a dictation.
//! Speeds are for a typical 6–15 s dictation on that CPU with 8 threads.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelKind {
    Speech,
    Rewrite,
}

#[derive(Clone, Debug)]
pub struct ModelInfo {
    pub kind: ModelKind,
    pub name: &'static str,
    pub file: &'static str,
    pub url: &'static str,
    pub size: u64,
    pub sha256: &'static str,
    /// Approximate RAM while loaded, in MB.
    pub ram_mb: u32,
    pub speed: &'static str,
    pub summary: &'static str,
    pub recommended: bool,
}

pub const MODELS: &[ModelInfo] = &[
    ModelInfo {
        kind: ModelKind::Speech,
        name: "Whisper Small (Q8)",
        file: "ggml-small-q8_0.bin",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small-q8_0.bin",
        size: 264_464_607,
        sha256: "49c8fb02b65e6049d5fa6c04f81f53b867b5ec9540406812c643f177317f779f",
        ram_mb: 450,
        speed: "≈0.4–0.6 s per sentence",
        summary: "Best balance of speed and accuracy on CPU. 99 languages.",
        recommended: true,
    },
    ModelInfo {
        kind: ModelKind::Speech,
        name: "Whisper Large v3 Turbo (Q5)",
        file: "ggml-large-v3-turbo-q5_0.bin",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin",
        size: 574_041_195,
        sha256: "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2",
        ram_mb: 780,
        speed: "≈2–4 s per sentence",
        summary: "Most accurate, especially for accents and non-English. Slower on CPU.",
        recommended: false,
    },
    ModelInfo {
        kind: ModelKind::Speech,
        name: "Whisper Small English (Q8)",
        file: "ggml-small.en-q8_0.bin",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.en-q8_0.bin",
        size: 264_477_561,
        sha256: "67a179f608ea6114bd3fdb9060e762b588a3fb3bd00c4387971be4d177958067",
        ram_mb: 450,
        speed: "≈0.4 s per sentence",
        summary: "English only; slightly more accurate than Small for English speakers.",
        recommended: false,
    },
    ModelInfo {
        kind: ModelKind::Speech,
        name: "Whisper Base",
        file: "ggml-base.bin",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin",
        size: 147_951_465,
        sha256: "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe",
        ram_mb: 250,
        speed: "≈0.2 s per sentence",
        summary: "For older or low-power PCs. Noticeably more mistakes.",
        recommended: false,
    },
    ModelInfo {
        kind: ModelKind::Rewrite,
        name: "Qwen 3.5 2B (Q4)",
        file: "Qwen3.5-2B-Q4_K_M.gguf",
        url: "https://huggingface.co/unsloth/Qwen3.5-2B-GGUF/resolve/main/Qwen3.5-2B-Q4_K_M.gguf",
        size: 1_280_835_840,
        sha256: "aaf42c8b7c3cab2bf3d69c355048d4a0ee9973d48f16c731c0520ee914699223",
        ram_mb: 1450,
        speed: "≈0.5–1 s per rewrite",
        summary: "Recommended: fast on CPU, multilingual, follows rewriting rules well.",
        recommended: true,
    },
    ModelInfo {
        kind: ModelKind::Rewrite,
        name: "Qwen 3.5 0.8B (Q4)",
        file: "Qwen3.5-0.8B-Q4_K_M.gguf",
        url: "https://huggingface.co/unsloth/Qwen3.5-0.8B-GGUF/resolve/main/Qwen3.5-0.8B-Q4_K_M.gguf",
        size: 532_517_120,
        sha256: "bd258782e35f7f458f8aced1adc053e6e92e89bc735ba3be89d38a06121dc517",
        ram_mb: 700,
        speed: "≈0.3 s per rewrite",
        summary: "Lightest option for 8 GB PCs. Handles cleanup well, weaker on restructuring.",
        recommended: false,
    },
    ModelInfo {
        kind: ModelKind::Rewrite,
        name: "Qwen 3.5 4B (Q4)",
        file: "Qwen3.5-4B-Q4_K_M.gguf",
        url: "https://huggingface.co/unsloth/Qwen3.5-4B-GGUF/resolve/main/Qwen3.5-4B-Q4_K_M.gguf",
        size: 2_740_937_888,
        sha256: "00fe7986ff5f6b463e62455821146049db6f9313603938a70800d1fb69ef11a4",
        ram_mb: 2900,
        speed: "≈1.5–2.5 s per rewrite",
        summary: "Higher quality for long, rambling dictation. Needs 16 GB RAM.",
        recommended: false,
    },
    ModelInfo {
        kind: ModelKind::Rewrite,
        name: "Llama 3.2 3B Instruct (Q4)",
        file: "Llama-3.2-3B-Instruct-Q4_K_M.gguf",
        url: "https://huggingface.co/bartowski/Llama-3.2-3B-Instruct-GGUF/resolve/main/Llama-3.2-3B-Instruct-Q4_K_M.gguf",
        size: 2_019_377_696,
        sha256: "6c1a2b41161032677be168d354123594c0e6e67d2b9227c84f296ad037c728ff",
        ram_mb: 2500,
        speed: "≈1–2 s per rewrite",
        summary: "Strong English writing; weaker than Qwen in Portuguese and other languages.",
        recommended: false,
    },
];

pub fn find(file: &str) -> Option<&'static ModelInfo> {
    MODELS.iter().find(|m| m.file.eq_ignore_ascii_case(file))
}

pub fn recommended() -> impl Iterator<Item = &'static ModelInfo> {
    MODELS.iter().filter(|m| m.recommended)
}

/// True if the model file is present with the expected size.
pub fn is_installed(model: &ModelInfo) -> bool {
    std::fs::metadata(crate::paths::models_dir().join(model.file)).map(|m| m.len() == model.size).unwrap_or(false)
}

pub fn format_size(bytes: u64) -> String {
    if bytes >= 1_000_000_000 {
        format!("{:.1} GB", bytes as f64 / 1e9)
    } else {
        format!("{:.0} MB", bytes as f64 / 1e6)
    }
}
