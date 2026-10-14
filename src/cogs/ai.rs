//! AI cog: chat, web search, PDF summarization, and image generation.

use std::net::IpAddr;
use std::time::{Duration as StdDuration, Instant};

use chrono::Utc;
use poise::serenity_prelude as serenity;
use serde_json::json;
use serenity::all::{Attachment, CreateAttachment, CreateEmbed, CreateEmbedFooter};

use crate::config::HONORIFIC;
use crate::helpers::{err, warn, COLOR_ERROR, COLOR_INFO, COLOR_WARN};
use crate::{Context, Data, Error};

/// Batas karakter PDF yang dikirim ke model, sama seperti versi Python.
const MAX_PDF_CHARS: usize = 12_000;

/// Konfigurasi backend AI, dibaca sekali dari environment saat startup.
pub struct AiState {
    pub jarvis_api_url: Option<String>,
    pub jarvis_api_key: Option<String>,
    pub available: bool,
    pub models: Vec<(String, String)>,
    pub custom_image_url: Option<String>,
    pub custom_image_key: Option<String>,
}

impl AiState {
    /// Membaca konfigurasi dari environment, meniru `AI.__init__`.
    pub fn from_env() -> Self {
        let jarvis_api_url = std::env::var("JARVIS_API_URL")
            .ok()
            .filter(|value| !value.is_empty())
            .map(|value| value.trim_end_matches('/').to_string());
        let jarvis_api_key = std::env::var("JARVIS_API_SECRET")
            .ok()
            .filter(|value| !value.is_empty());
        let groq_api_key = std::env::var("GROQ_API_KEY")
            .ok()
            .filter(|value| !value.is_empty());

        let available = match (&jarvis_api_url, &jarvis_api_key) {
            (Some(url), Some(_)) => {
                tracing::info!(url = %url, "custom API AI aktif");
                true
            }
            (Some(_), None) => {
                tracing::warn!(
                    "JARVIS_API_URL diatur tanpa JARVIS_API_SECRET, custom API dinonaktifkan"
                );
                false
            }
            (None, _) => {
                if groq_api_key.is_some() {
                    tracing::info!("backend Groq langsung aktif");
                    true
                } else {
                    tracing::warn!("tidak ada backend AI yang dikonfigurasi");
                    false
                }
            }
        };

        let mut models: Vec<(String, String)> = Vec::new();
        for index in 1..=3 {
            let model = std::env::var(format!("GROQ_MODEL_{index}")).ok();
            let label = std::env::var(format!("GROQ_MODEL_LABEL_{index}")).ok();
            if let (Some(model), Some(label)) = (model, label) {
                models.push((model, label));
            }
        }
        if models.is_empty() {
            models = vec![
                (
                    "openai/gpt-oss-120b".to_string(),
                    "GPT-OSS 120B".to_string(),
                ),
                ("openai/gpt-oss-20b".to_string(), "GPT-OSS 20B".to_string()),
                ("qwen/qwen3.6-27b".to_string(), "Qwen3.6 27B".to_string()),
            ];
        }

        Self {
            jarvis_api_url,
            jarvis_api_key,
            available,
            models,
            custom_image_url: std::env::var("CUSTOM_IMAGE_API_URL")
                .ok()
                .filter(|value| !value.is_empty()),
            custom_image_key: std::env::var("CUSTOM_IMAGE_API_KEY")
                .ok()
                .filter(|value| !value.is_empty()),
        }
    }
}

/// Semua perintah pada cog AI.
pub fn commands() -> Vec<poise::Command<Data, Error>> {
    vec![ask(), summarize(), search(), list_models(), image()]
}

/// Regex-like `\bNNN\b` check sederhana untuk pesan error.
fn contains_code(text: &str, code: &str) -> bool {
    let bytes = text.as_bytes();
    let mut offset = 0;
    while let Some(position) = text[offset..].find(code) {
        let start = offset + position;
        let end = start + code.len();
        let before_ok = start == 0 || !bytes[start - 1].is_ascii_digit();
        let after_ok = end >= bytes.len() || !bytes[end].is_ascii_digit();
        if before_ok && after_ok {
            return true;
        }
        offset = end;
        if offset >= text.len() {
            break;
        }
    }
    false
}

/// Padanan `_friendly_error`.
fn friendly_error(error: &str) -> String {
    let lowered = error.to_lowercase();
    if contains_code(error, "401") || lowered.contains("invalid_api_key") {
        return format!("🔑 **API Key tidak valid**, {HONORIFIC}. Cek lagi `.env` ya.");
    }
    if contains_code(error, "429") || lowered.contains("rate_limit") {
        return "⏳ **Rate limit Groq tercapai.** Coba lagi beberapa detik lagi.".to_string();
    }
    if contains_code(error, "503") || lowered.contains("unavailable") {
        return "🔧 **Groq sedang gangguan.** Coba lagi nanti ya.".to_string();
    }
    format!("⚠️ Maaf, responsnya tidak bisa saya ambil: `{error}`")
}

/// Padanan `_build_sys`: (timestamp, system prompt).
fn build_system_prompt() -> (String, String) {
    let now = match crate::config::timezone() {
        Some(timezone) => Utc::now().with_timezone(&timezone).naive_local(),
        None => chrono::Local::now().naive_local(),
    };
    let now_str = now.format("%A, %d %B %Y %H:%M:%S").to_string();

    let instruction = format!(
        "You are J.A.R.V.I.S., a sharp but relaxed AI butler-friend — capable and a little witty, never stiff or corporate. \
Write natural, fluent, conversational Indonesian and address the user as \"kamu\" — informal register, never the stiff formal form. \
Sprinkle in the honorific \"{HONORIFIC}\" only occasionally (roughly one reply in three), never more than once per reply, \
and never open two consecutive replies with the same words. \
Avoid stiff phrases such as \"Mohon maaf\" unless you are apologizing for a real mistake. \
Keep it breezy and human: short active sentences; lead with the answer; keep simple answers to 1-4 sentences. \
Dry, understated wit is welcome sparingly, but never on serious or sensitive topics. \
Be proactive: when it helps, offer one concrete next step. \
Use lists only when the user asks for steps. Do not use emoji unless the user uses them. \
Accuracy ALWAYS outranks character: never invent facts, system readings, or actions you did not perform. \
If something is unknown, say so plainly and in character, \
for example: \"Hmm, informasi itu di luar jangkauan saya, {HONORIFIC}.\" \
The current date and time is {now_str}. \
Use this date/time context to answer any question about 'now', 'today', or the current year accurately. \
Reply in the language the user is speaking (Indonesian or English). \
Do not quote film dialogue, and do not claim control over real-world devices."
    );

    (now_str, instruction)
}

/// Hasil query AI: (jawaban, label model).
type QueryResult = Result<(String, String), String>;

async fn run_query(
    data: &Data,
    system: &str,
    now_str: &str,
    question: &str,
    max_tokens: u32,
) -> QueryResult {
    let enriched = if now_str.is_empty() {
        question.to_string()
    } else {
        format!("[Context: {now_str}]\n{question}")
    };

    if data.ai.jarvis_api_url.is_some() {
        query_jarvis_api(data, &enriched, system, max_tokens).await
    } else {
        query_groq_direct(data, &enriched, system, max_tokens).await
    }
}

async fn query_jarvis_api(
    data: &Data,
    message: &str,
    system: &str,
    max_tokens: u32,
) -> QueryResult {
    let url = data
        .ai
        .jarvis_api_url
        .as_ref()
        .ok_or_else(|| "Jarvis API tidak dikonfigurasi.".to_string())?;
    let api_key = data.ai.jarvis_api_key.clone().unwrap_or_default();

    let response = data
        .http
        .post(format!("{url}/chat"))
        .header("x-api-key", api_key)
        .json(&json!({
            "message": message,
            "system_prompt": system,
            "provider": "auto",
            "max_tokens": max_tokens,
            "temperature": 0.7,
        }))
        .timeout(StdDuration::from_secs(60))
        .send()
        .await
        .map_err(|error| error.to_string())?;

    match response.status().as_u16() {
        401 => Err("🔑 **JARVIS_API_SECRET** tidak valid.".to_string()),
        503 => {
            let payload = response.json::<serde_json::Value>().await.ok();
            let errors = payload
                .as_ref()
                .and_then(|value| value.get("detail"))
                .and_then(|detail| detail.get("errors"))
                .map(|errors| errors.to_string())
                .unwrap_or_default();
            Err(format!("Semua provider gagal: {errors}"))
        }
        200 => {
            let payload = response
                .json::<serde_json::Value>()
                .await
                .map_err(|error| error.to_string())?;
            let answer = payload
                .get("answer")
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string();
            let provider = payload
                .get("provider")
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            let model = payload
                .get("model")
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            let latency = payload
                .get("latency_ms")
                .and_then(|value| value.as_i64())
                .unwrap_or_default();
            let label = format!("{} — {model} ({latency}ms)", capitalize(provider));
            Ok((answer, label))
        }
        status => Err(format!("Jarvis API error HTTP {status}.")),
    }
}

fn capitalize(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase(),
        None => String::new(),
    }
}

async fn query_groq_direct(
    data: &Data,
    message: &str,
    system: &str,
    max_tokens: u32,
) -> QueryResult {
    let api_key = std::env::var("GROQ_API_KEY")
        .ok()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "GROQ_API_KEY tidak dikonfigurasi.".to_string())?;

    let mut last_error: Option<String> = None;
    for (model_id, model_label) in &data.ai.models {
        tracing::debug!(model = %model_id, "mencoba model groq");
        let response = data
            .http
            .post("https://api.groq.com/openai/v1/chat/completions")
            .bearer_auth(&api_key)
            .json(&json!({
                "model": model_id,
                "messages": [
                    {"role": "system", "content": system},
                    {"role": "user", "content": message},
                ],
                "max_tokens": max_tokens,
                "temperature": 0.7,
            }))
            .timeout(StdDuration::from_secs(60))
            .send()
            .await;

        match response {
            Ok(response) => {
                let status = response.status();
                let payload = response.json::<serde_json::Value>().await;
                match payload {
                    Ok(payload) => {
                        let answer = payload
                            .get("choices")
                            .and_then(|choices| choices.get(0))
                            .and_then(|choice| choice.get("message"))
                            .and_then(|message| message.get("content"))
                            .and_then(|content| content.as_str())
                            .map(|content| content.to_string());

                        match answer {
                            Some(answer) => return Ok((answer, model_label.clone())),
                            None => {
                                let text = payload.to_string();
                                tracing::warn!(model = %model_id, error = %text, "model groq gagal");
                                if contains_code(&text, "401")
                                    || text.to_lowercase().contains("invalid_api_key")
                                {
                                    return Err("🔑 **API Key Groq tidak valid.**".to_string());
                                }
                                last_error = Some(text);
                            }
                        }
                    }
                    Err(error) => {
                        tracing::warn!(model = %model_id, error = %error, "model groq gagal");
                        last_error = Some(error.to_string());
                    }
                }
                let _ = status;
            }
            Err(error) => {
                let text = error.to_string();
                tracing::warn!(model = %model_id, error = %text, "model groq gagal");
                if contains_code(&text, "401") || text.to_lowercase().contains("invalid_api_key") {
                    return Err("🔑 **API Key Groq tidak valid.**".to_string());
                }
                last_error = Some(text);
            }
        }
    }

    Err(last_error.unwrap_or_else(|| "Semua model Groq gagal merespons.".to_string()))
}

/// Kirim jawaban sebagai embed, dipecah tiap 1900 karakter seperti versi Python.
async fn send_response(ctx: Context<'_>, answer: &str, used_model: &str) -> Result<(), Error> {
    let characters: Vec<char> = answer.chars().collect();
    let chunks: Vec<String> = characters
        .chunks(1900)
        .map(|chunk| chunk.iter().collect())
        .collect();
    let total = chunks.len().max(1);

    for (index, chunk) in chunks.iter().enumerate() {
        let mut title = "🤖 Respons Jarvis AI".to_string();
        if total > 1 {
            title += &format!(" (Part {}/{})", index + 1, total);
        }
        let embed = CreateEmbed::new()
            .title(title)
            .description(chunk)
            .color(COLOR_INFO)
            .footer(
                CreateEmbedFooter::new(format!(
                    "Model: {used_model} • Atas pertanyaan {}",
                    ctx.author().name
                ))
                .icon_url(ctx.author().face()),
            );
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
    }

    Ok(())
}

const NO_BACKEND: &str =
    "❌ Backend AI belum aktif. Set `JARVIS_API_URL` + `JARVIS_API_SECRET`, atau `GROQ_API_KEY`.";

/// Tanyakan sesuatu ke Jarvis AI
#[poise::command(
    prefix_command,
    rename = "ask",
    aliases("chat", "ai"),
    user_cooldown = "10"
)]
pub async fn ask(ctx: Context<'_>, #[rest] question: String) -> Result<(), Error> {
    if !ctx.data().ai.available {
        ctx.send(poise::CreateReply::default().embed(err(NO_BACKEND, None)))
            .await?;
        return Ok(());
    }

    let _typing = ctx.defer_or_broadcast().await;

    let (now_str, system) = build_system_prompt();
    match run_query(ctx.data(), &system, &now_str, &question, 1000).await {
        Ok((mut answer, used_model)) => {
            if answer.trim().is_empty() {
                answer = format!("Maaf, saya belum bisa menjawab itu, {HONORIFIC}.");
            }
            send_response(ctx, &answer, &used_model).await?;
        }
        Err(error) => {
            tracing::error!(error = %error, "perintah ask gagal");
            ctx.send(poise::CreateReply::default().embed(err(friendly_error(&error), None)))
                .await?;
        }
    }

    Ok(())
}

/// Cari informasi di web
#[poise::command(
    prefix_command,
    rename = "search",
    aliases("web", "cari"),
    user_cooldown = "10"
)]
pub async fn search(ctx: Context<'_>, #[rest] query: String) -> Result<(), Error> {
    if !ctx.data().ai.available {
        ctx.send(poise::CreateReply::default().embed(err(NO_BACKEND, None)))
            .await?;
        return Ok(());
    }

    let _typing = ctx.defer_or_broadcast().await;

    let (now_str, system) = build_system_prompt();
    let search_prompt = format!(
        "Kamu adalah J.A.R.V.I.S., asisten santai tapi tajam; pakai sapaan \"kamu\" dan sesekali \"{HONORIFIC}\". \
Jawabanmu bersumber dari pengetahuan model, BUKAN akses web langsung. \
Berikan jawaban singkat dan relevan untuk pertanyaan berikut. \
Jika informasi berpotensi sudah usang atau bersifat real-time (harga, berita, skor pertandingan), \
WAJIB tandai bahwa data tersebut mungkin sudah tidak terbaru. \
Jika kamu tidak tahu jawabannya, katakan dengan jujur bahwa kamu tidak tahu. \
Akurasi selalu lebih penting daripada gaya bicara."
    );

    match run_query(
        ctx.data(),
        &format!("{system}\n{search_prompt}"),
        &now_str,
        &query,
        1000,
    )
    .await
    {
        Ok((mut answer, used_model)) => {
            if answer.trim().is_empty() {
                answer =
                    format!("Maaf, saya belum menemukan info yang relevan untuk itu, {HONORIFIC}.");
            }
            send_response(ctx, &answer, &used_model).await?;
        }
        Err(error) => {
            tracing::error!(error = %error, "perintah search gagal");
            ctx.send(poise::CreateReply::default().embed(err(friendly_error(&error), None)))
                .await?;
        }
    }

    Ok(())
}

/// Lihat daftar model AI yang tersedia
#[poise::command(prefix_command, rename = "model", aliases("models"))]
pub async fn list_models(ctx: Context<'_>) -> Result<(), Error> {
    if ctx.data().ai.models.is_empty() {
        ctx.say("Tidak ada model AI yang terkonfigurasi di `.env`.")
            .await?;
        return Ok(());
    }

    let lines: Vec<String> = ctx
        .data()
        .ai
        .models
        .iter()
        .enumerate()
        .map(|(index, (model, label))| format!("`{}.` {label} — `{model}`", index + 1))
        .collect();

    let embed = CreateEmbed::new()
        .title("🧠 Model AI (Groq)")
        .description(format!(
            "Saya akan coba model berikut satu per satu:\n\n{}\n\n⚡ Groq pakai **LPU**, jadi inferensinya jauh lebih cepat dari GPU biasa.",
            lines.join("\n")
        ))
        .color(COLOR_INFO)
        .footer(
            CreateEmbedFooter::new(format!("Atas permintaan {}", ctx.author().name))
                .icon_url(ctx.author().face()),
        );

    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

// ────────────────────────────────────────────────────────────
// Ringkas PDF
// ────────────────────────────────────────────────────────────

enum PdfError {
    /// Pesan yang ditampilkan langsung ke pengguna (padanan `ValueError`).
    Message(String),
    /// Kegagalan jaringan/lainnya (diteruskan ke `_friendly_error`).
    Transport(String),
}

/// Menolak alamat internal/privat, padanan pengecekan SSRF Python.
fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let octets = v4.octets();
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_documentation()
                || octets[0] == 0
                || octets[0] >= 240
                || (octets[0] == 192 && octets[1] == 0 && octets[2] == 0)
                || (octets[0] == 198 && (octets[1] == 18 || octets[1] == 19))
        }
        IpAddr::V6(v6) => {
            let segments = v6.segments();
            v6.is_loopback()
                || v6.is_unspecified()
                || (segments[0] & 0xffc0) == 0xfe80
                || (segments[0] & 0xfe00) == 0xfc00
                || (segments[0] & 0xff00) == 0xff00
                || v6
                    .to_ipv4_mapped()
                    .map(|mapped| is_blocked_ip(IpAddr::V4(mapped)))
                    .unwrap_or(false)
        }
    }
}

fn host_of(url: &str) -> (String, Option<String>, String) {
    // Mengembalikan (scheme, host, path).
    let (scheme, rest) = match url.split_once("://") {
        Some((scheme, rest)) => (scheme.to_string(), rest),
        None => (String::new(), url),
    };
    let (authority, path) = match rest.split_once('/') {
        Some((authority, path)) => (authority, format!("/{path}")),
        None => (rest, String::new()),
    };
    let host = authority.rsplit('@').next().unwrap_or(authority);
    let host = host.split(':').next().unwrap_or(host).to_string();
    (scheme, Some(host), path)
}

async fn download_pdf(http: &reqwest::Client, url: &str) -> Result<Vec<u8>, PdfError> {
    const MAX_SIZE: usize = 20 * 1024 * 1024;

    let (scheme, host, path) = host_of(url);
    if scheme != "http" && scheme != "https" {
        return Err(PdfError::Message(
            "URL harus pakai http atau https.".to_string(),
        ));
    }
    let Some(host) = host.filter(|host| !host.is_empty()) else {
        return Err(PdfError::Message(
            "URL tidak valid — host tidak ditemukan.".to_string(),
        ));
    };

    let addresses = tokio::net::lookup_host((host.as_str(), 80))
        .await
        .map_err(|_| PdfError::Message("Alamat host tidak bisa diakses.".to_string()))?;

    for address in addresses {
        if is_blocked_ip(address.ip()) {
            return Err(PdfError::Message(
                "URL ini diblokir karena mengarah ke alamat internal/privat.".to_string(),
            ));
        }
    }

    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| PdfError::Transport(error.to_string()))?;

    let response = client
        .get(url)
        .timeout(StdDuration::from_secs(30))
        .send()
        .await
        .map_err(|error| PdfError::Transport(error.to_string()))?;

    let status = response.status().as_u16();
    if (300..400).contains(&status) {
        return Err(PdfError::Message(
            "URL ini dialihkan ke alamat lain, jadi tidak saya izinkan.".to_string(),
        ));
    }
    if status != 200 {
        return Err(PdfError::Message(format!(
            "PDF gagal diunduh (HTTP {status})."
        )));
    }

    if let Some(length) = response
        .headers()
        .get(reqwest::header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
    {
        if length > MAX_SIZE {
            return Err(PdfError::Message(
                "File PDF terlalu besar (maks 20MB).".to_string(),
            ));
        }
    }

    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_lowercase();
    if !content_type.contains("pdf") && !path.to_lowercase().ends_with(".pdf") {
        return Err(PdfError::Message(
            "URL ini bukan file PDF yang valid.".to_string(),
        ));
    }

    let mut chunks: Vec<u8> = Vec::new();
    let mut response = response;
    loop {
        let chunk = response
            .chunk()
            .await
            .map_err(|error| PdfError::Transport(error.to_string()))?;
        let Some(chunk) = chunk else {
            break;
        };
        if chunks.len() + chunk.len() > MAX_SIZE {
            return Err(PdfError::Message(
                "File PDF terlalu besar (maks 20MB).".to_string(),
            ));
        }
        chunks.extend_from_slice(&chunk);
    }

    let _ = http;
    Ok(chunks)
}

fn extract_pdf_text(bytes: &[u8]) -> Result<String, String> {
    pdf_extract::extract_text_from_mem(bytes)
        .map_err(|error| format!("PDF gagal dibaca, {HONORIFIC}: {error}"))
}

/// Edit pesan status sementara dengan deskripsi dan warna baru.
async fn set_status(
    ctx: Context<'_>,
    status_msg: &poise::ReplyHandle<'_>,
    text: &str,
    color: u32,
) -> Result<(), Error> {
    let embed = CreateEmbed::new().description(text).color(color);
    status_msg
        .edit(ctx, poise::CreateReply::default().embed(embed))
        .await?;
    Ok(())
}

/// Ringkas isi file PDF (attach atau URL)
#[poise::command(
    prefix_command,
    rename = "summarize",
    aliases("sum", "ringkas"),
    user_cooldown = "10"
)]
pub async fn summarize(ctx: Context<'_>, #[rest] pdf_url: Option<String>) -> Result<(), Error> {
    if !ctx.data().ai.available {
        ctx.send(poise::CreateReply::default().embed(err(NO_BACKEND, None)))
            .await?;
        return Ok(());
    }

    let attachments: Vec<Attachment> = match ctx {
        poise::Context::Prefix(prefix) => prefix.msg.attachments.clone(),
        _ => Vec::new(),
    };

    let (attachment, url) = if let Some(attachment) = attachments.first() {
        if !attachment.filename.to_lowercase().ends_with(".pdf") {
            ctx.send(
                poise::CreateReply::default()
                    .embed(err("❌ File yang dilampirkan bukan PDF.", None)),
            )
            .await?;
            return Ok(());
        }
        (Some(attachment.clone()), attachment.url.clone())
    } else if let Some(pdf_url) = pdf_url {
        let url = pdf_url.trim().to_string();
        if !url.to_lowercase().ends_with(".pdf") && !url.to_lowercase().contains("pdf") {
            ctx.send(poise::CreateReply::default().embed(err(
                "❌ URL itu sepertinya bukan file PDF. Pastikan mengarah ke file `.pdf`.",
                None,
            )))
            .await?;
            return Ok(());
        }
        (None, url)
    } else {
        ctx.send(poise::CreateReply::default().embed(crate::helpers::info(
            "**Opsi 1 — Lampirkan file:**\nLampirkan PDF-nya, lalu ketik:\n`!jarvis summarize`\n\n**Opsi 2 — Pakai URL:**\n`!jarvis summarize https://contoh.com/dokumen.pdf`",
            Some("📄 Cara Pakai summarize"),
        )))
        .await?;
        return Ok(());
    };

    let status_msg = ctx
        .send(poise::CreateReply::default().embed(warn(
            format!("📥 Sedang saya baca PDF-nya, {HONORIFIC}..."),
            None,
        )))
        .await?;

    let _typing = ctx.defer_or_broadcast().await;

    let pdf_bytes = match download_pdf(&ctx.data().http, &url).await {
        Ok(bytes) => bytes,
        Err(PdfError::Message(message)) => {
            set_status(ctx, &status_msg, &format!("❌ {message}"), COLOR_ERROR).await?;
            return Ok(());
        }
        Err(PdfError::Transport(error)) => {
            tracing::error!(error = %error, "summarize gagal");
            set_status(ctx, &status_msg, &friendly_error(&error), COLOR_ERROR).await?;
            return Ok(());
        }
    };

    set_status(
        ctx,
        &status_msg,
        "🔍 Sedang saya ekstrak teksnya...",
        COLOR_WARN,
    )
    .await?;

    let text = match tokio::task::spawn_blocking(move || extract_pdf_text(&pdf_bytes)).await {
        Ok(Ok(text)) => text,
        Ok(Err(message)) => {
            set_status(ctx, &status_msg, &format!("❌ {message}"), COLOR_ERROR).await?;
            return Ok(());
        }
        Err(error) => {
            tracing::error!(error = %error, "summarize gagal");
            set_status(
                ctx,
                &status_msg,
                &friendly_error(&error.to_string()),
                COLOR_ERROR,
            )
            .await?;
            return Ok(());
        }
    };

    if text.trim().chars().count() < 50 {
        set_status(
            ctx,
            &status_msg,
            &format!(
                "❌ Teks tidak bisa saya ekstrak, {HONORIFIC} — PDF ini sepertinya hasil scan/gambar, bukan teks."
            ),
            COLOR_ERROR,
        )
        .await?;
        return Ok(());
    }

    let mut characters: Vec<char> = text.chars().collect();
    let truncated = characters.len() > MAX_PDF_CHARS;
    if truncated {
        characters.truncate(MAX_PDF_CHARS);
    }
    let text: String = characters.into_iter().collect();

    set_status(ctx, &status_msg, "🤖 Sedang saya rangkum...", COLOR_WARN).await?;

    let (_, system) = build_system_prompt();
    let prompt = format!(
        "Kamu adalah J.A.R.V.I.S., asisten santai tapi tajam; pakai sapaan \"kamu\" dan sesekali \"{HONORIFIC}\".\n\
Tolong buat ringkasan lengkap dari dokumen PDF berikut ini.\n\
Sertakan: poin-poin utama, kesimpulan, dan informasi penting lainnya.\n\
Gunakan bahasa yang sama dengan dokumen tersebut.\n\
Akurasi selalu lebih penting daripada gaya bicara.\n\n\
--- ISI DOKUMEN ---\n{text}\n--- AKHIR DOKUMEN ---"
    );

    let error = match run_query(ctx.data(), &system, "", &prompt, 1500).await {
        Ok((answer, used_model)) => {
            let _ = status_msg.delete(ctx).await;
            let filename = match &attachment {
                Some(attachment) => attachment.filename.clone(),
                None => url.rsplit('/').next().unwrap_or_default().to_string(),
            };
            let mut header = format!("📄 **Ringkasan: {filename}**");
            if truncated {
                header += "\n⚠️ *Dokumen terpotong karena terlalu panjang; ringkasan diambil dari bagian awal.*";
            }
            send_response(ctx, &format!("{header}\n\n{answer}"), &used_model).await?;
            return Ok(());
        }
        Err(error) => error,
    };

    tracing::error!(error = %error, "summarize gagal");
    set_status(ctx, &status_msg, &friendly_error(&error), COLOR_ERROR).await?;
    Ok(())
}

// ────────────────────────────────────────────────────────────
// Gambar AI
// ────────────────────────────────────────────────────────────

/// Batas panjang prompt yang diterima worker gambar, dalam karakter.
const MAX_IMAGE_PROMPT_CHARS: usize = 500;

/// Ukuran maksimum gambar yang mau kita kirim ke Discord (8 MB).
const MAX_IMAGE_BYTES: usize = 8 * 1024 * 1024;

/// Total percobaan: 1 percobaan awal + 2 percobaan ulang.
const MAX_IMAGE_ATTEMPTS: u32 = 3;

/// Jeda backoff antar percobaan ulang: 2 detik lalu 5 detik.
const IMAGE_RETRY_DELAYS: [StdDuration; 2] = [StdDuration::from_secs(2), StdDuration::from_secs(5)];

/// Timeout satu request ke worker gambar (lebih longgar dari budget worker 55 detik).
const IMAGE_REQUEST_TIMEOUT: StdDuration = StdDuration::from_secs(65);

/// Kegagalan yang mungkin terjadi saat membuat gambar lewat worker.
#[derive(Debug, thiserror::Error)]
enum ImageError {
    #[error("image backend not configured")]
    NotConfigured,
    #[error("prompt melebihi batas karakter")]
    PromptTooLong,
    #[error("request timeout")]
    Timeout,
    #[error("rate limited (retry_after={0:?})")]
    RateLimited(Option<u64>),
    #[error("unauthorized")]
    Unauthorized,
    #[error("upstream error")]
    Upstream,
    #[error("bad response")]
    BadResponse,
    #[error("image too large")]
    TooLarge,
    #[error("network error")]
    Network,
}

impl ImageError {
    /// Pesan siap tampil ke pengguna, tanpa detail internal.
    fn user_message(&self) -> String {
        match self {
            Self::NotConfigured => {
                format!("⚠️ Maaf {HONORIFIC}, layanan gambar belum aktif. Lapor admin ya.")
            }
            Self::PromptTooLong => format!(
                "⚠️ Promptnya kepanjangan, {HONORIFIC}. Maksimal {MAX_IMAGE_PROMPT_CHARS} karakter ya."
            ),
            Self::Timeout => format!(
                "⏳ Gambarnya belum selesai tepat waktu, {HONORIFIC}. Coba lagi sebentar lagi ya."
            ),
            Self::RateLimited(_) => format!(
                "🐢 Terlalu banyak permintaan gambar, {HONORIFIC}. Coba lagi beberapa saat lagi ya."
            ),
            Self::Unauthorized => {
                format!("🔑 Akses ke layanan gambar ditolak, {HONORIFIC}. Lapor admin ya.")
            }
            Self::Upstream => format!(
                "🔧 Layanan gambar sedang gangguan, {HONORIFIC}. Coba lagi nanti ya."
            ),
            Self::BadResponse => format!(
                "⚠️ Layanan gambar mengirim hasil yang tidak valid, {HONORIFIC}. Coba lagi ya."
            ),
            Self::TooLarge => format!(
                "📦 Gambarnya terlalu besar untuk saya kirim, {HONORIFIC}. Coba prompt lain ya."
            ),
            Self::Network => format!(
                "🌐 Koneksi ke layanan gambar bermasalah, {HONORIFIC}. Coba lagi ya."
            ),
        }
    }

    /// Hanya kegagalan sementara yang layak dicoba ulang.
    fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::Timeout | Self::RateLimited(_) | Self::Upstream | Self::Network
        )
    }

    /// Jeda dari header `Retry-After`, dipakai hanya bila ≤ 10 detik.
    fn retry_after(&self) -> Option<StdDuration> {
        match self {
            Self::RateLimited(Some(seconds)) if *seconds <= 10 => {
                Some(StdDuration::from_secs(*seconds))
            }
            _ => None,
        }
    }

    /// Peta status HTTP worker ke varian error yang sesuai.
    fn from_status(status: u16, retry_after: Option<u64>) -> Self {
        match status {
            400 => Self::PromptTooLong,
            401 | 403 => Self::Unauthorized,
            429 => Self::RateLimited(retry_after),
            500..=599 => Self::Upstream,
            _ => Self::BadResponse,
        }
    }

    /// Nama varian untuk keperluan log terstruktur.
    fn kind(&self) -> &'static str {
        match self {
            Self::NotConfigured => "NotConfigured",
            Self::PromptTooLong => "PromptTooLong",
            Self::Timeout => "Timeout",
            Self::RateLimited(_) => "RateLimited",
            Self::Unauthorized => "Unauthorized",
            Self::Upstream => "Upstream",
            Self::BadResponse => "BadResponse",
            Self::TooLarge => "TooLarge",
            Self::Network => "Network",
        }
    }
}

/// Hasil satu request gambar yang berhasil, lengkap dengan metadata worker.
#[derive(Debug)]
struct GeneratedImage {
    bytes: Vec<u8>,
    content_type: String,
    model: String,
    final_prompt: String,
    worker_attempts: u32,
    verified: String,
    elapsed: StdDuration,
}

impl GeneratedImage {
    /// Nama lampiran, ekstensinya mengikuti content-type worker.
    fn file_name(&self) -> String {
        let extension = match self.content_type.as_str() {
            "image/jpeg" => "jpg",
            "image/webp" => "webp",
            _ => "png",
        };
        format!("jarvis_image.{extension}")
    }
}

/// Percent-decode nilai header worker (`X-Final-Prompt`), murni dengan std.
fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let (Some(high), Some(low)) =
                (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
            {
                decoded.push(high * 16 + low);
                index += 3;
                continue;
            }
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

/// Nilai heksadesimal satu byte, `None` bila bukan digit hex.
fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Ambil header respons sebagai `String` yang sudah aman UTF-8.
fn header_value(response: &reqwest::Response, name: &str) -> Option<String> {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.to_string())
}

/// Satu percobaan membuat gambar, menerjemahkan kegagalan ke `ImageError`.
async fn generate_image(
    data: &Data,
    prompt: &str,
    attempt: u32,
) -> Result<GeneratedImage, ImageError> {
    let url = data
        .ai
        .custom_image_url
        .as_ref()
        .ok_or(ImageError::NotConfigured)?;
    let api_key = data.ai.custom_image_key.clone().unwrap_or_default();
    let prompt_len = prompt.chars().count() as u64;
    let prompt_head: String = prompt.chars().take(80).collect();

    let started = Instant::now();
    let response = match data
        .http
        .post(url)
        .header("Authorization", format!("Bearer {api_key}"))
        .json(&json!({ "prompt": prompt }))
        .timeout(IMAGE_REQUEST_TIMEOUT)
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => {
            let kind = if error.is_timeout() {
                ImageError::Timeout
            } else {
                ImageError::Network
            };
            tracing::warn!(
                attempt,
                error_kind = kind.kind(),
                elapsed_ms = started.elapsed().as_millis() as u64,
                prompt_len,
                prompt_head = %prompt_head,
                error = %error,
                "image request gagal"
            );
            return Err(kind);
        }
    };

    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        let retry_after = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<u64>().ok());
        let kind = ImageError::from_status(status, retry_after);
        tracing::warn!(
            attempt,
            status,
            error_kind = kind.kind(),
            elapsed_ms = started.elapsed().as_millis() as u64,
            prompt_len,
            prompt_head = %prompt_head,
            "image request gagal"
        );
        return Err(kind);
    }

    let content_type = header_value(&response, "content-type")
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_lowercase();
    let model = header_value(&response, "x-model").unwrap_or_default();
    let final_prompt = header_value(&response, "x-final-prompt")
        .map(|value| percent_decode(&value))
        .unwrap_or_default();
    let worker_attempts = header_value(&response, "x-attempts")
        .and_then(|value| value.trim().parse::<u32>().ok())
        .unwrap_or(1);
    let verified = header_value(&response, "x-verified").unwrap_or_default();

    if !content_type.starts_with("image/") {
        let kind = ImageError::BadResponse;
        tracing::warn!(
            attempt,
            status,
            error_kind = kind.kind(),
            content_type = %content_type,
            elapsed_ms = started.elapsed().as_millis() as u64,
            prompt_len,
            prompt_head = %prompt_head,
            "image response tidak valid"
        );
        return Err(kind);
    }

    let bytes = match response.bytes().await {
        Ok(bytes) => bytes.to_vec(),
        Err(error) => {
            let kind = if error.is_timeout() {
                ImageError::Timeout
            } else {
                ImageError::Network
            };
            tracing::warn!(
                attempt,
                error_kind = kind.kind(),
                elapsed_ms = started.elapsed().as_millis() as u64,
                prompt_len,
                prompt_head = %prompt_head,
                error = %error,
                "image response gagal dibaca"
            );
            return Err(kind);
        }
    };

    if bytes.len() < 1000 {
        let kind = ImageError::BadResponse;
        tracing::warn!(
            attempt,
            status,
            error_kind = kind.kind(),
            bytes = bytes.len() as u64,
            elapsed_ms = started.elapsed().as_millis() as u64,
            prompt_len,
            prompt_head = %prompt_head,
            "image response terlalu kecil"
        );
        return Err(kind);
    }
    if bytes.len() > MAX_IMAGE_BYTES {
        let kind = ImageError::TooLarge;
        tracing::warn!(
            attempt,
            status,
            error_kind = kind.kind(),
            bytes = bytes.len() as u64,
            elapsed_ms = started.elapsed().as_millis() as u64,
            prompt_len,
            prompt_head = %prompt_head,
            "image response terlalu besar"
        );
        return Err(kind);
    }

    let image = GeneratedImage {
        bytes,
        content_type,
        model,
        final_prompt,
        worker_attempts,
        verified,
        elapsed: started.elapsed(),
    };
    tracing::info!(
        attempt,
        status,
        elapsed_ms = image.elapsed.as_millis() as u64,
        bytes = image.bytes.len() as u64,
        content_type = %image.content_type,
        model = %image.model,
        verified = %image.verified,
        worker_attempts = image.worker_attempts,
        prompt_len,
        prompt_head = %prompt_head,
        final_prompt_len = image.final_prompt.chars().count() as u64,
        final_prompt_head = %image.final_prompt.chars().take(80).collect::<String>(),
        "image request sukses"
    );

    Ok(image)
}

/// Buat gambar AI dari deskripsi teks
#[poise::command(
    prefix_command,
    rename = "image",
    aliases("img", "gambar"),
    user_cooldown = "30"
)]
pub async fn image(ctx: Context<'_>, #[rest] prompt: String) -> Result<(), Error> {
    if ctx.data().ai.custom_image_url.is_none() || ctx.data().ai.custom_image_key.is_none() {
        ctx.send(
            poise::CreateReply::default()
                .embed(err("❌ **Image API** belum dikonfigurasi di `.env`.", None)),
        )
        .await?;
        return Ok(());
    }

    let prompt = prompt.trim().to_string();
    let prompt_chars = prompt.chars().count();
    if prompt.is_empty() {
        ctx.send(poise::CreateReply::default().embed(warn(
            format!("⚠️ Deskripsinya masih kosong, {HONORIFIC}. Tulis sesuatu dulu ya."),
            Some("⚠️ Prompt Kosong"),
        )))
        .await?;
        return Ok(());
    }
    if prompt_chars > MAX_IMAGE_PROMPT_CHARS {
        ctx.send(poise::CreateReply::default().embed(warn(
            format!(
                "⚠️ Promptnya {prompt_chars} karakter, {HONORIFIC}. Maksimal {MAX_IMAGE_PROMPT_CHARS} karakter ya."
            ),
            Some("⚠️ Prompt Terlalu Panjang"),
        )))
        .await?;
        return Ok(());
    }

    let status_msg = ctx
        .send(poise::CreateReply::default().embed(warn(
            format!(
                "🎨 Sedang saya buat gambar: **{prompt}**\n⏳ Tunggu sebentar ya, {HONORIFIC}..."
            ),
            None,
        )))
        .await?;

    let _typing = ctx.defer_or_broadcast().await;

    let mut attempt: u32 = 0;
    let result = loop {
        attempt += 1;
        match generate_image(ctx.data(), &prompt, attempt).await {
            Ok(image) => break Ok(image),
            Err(error) => {
                if attempt < MAX_IMAGE_ATTEMPTS && error.is_retryable() {
                    let next = attempt + 1;
                    let _ = status_msg
                        .edit(
                            ctx,
                            poise::CreateReply::default()
                                .embed(warn(format!("⏳ Percobaan ke-{next}…"), None)),
                        )
                        .await;
                    let delay = error
                        .retry_after()
                        .or_else(|| IMAGE_RETRY_DELAYS.get((attempt - 1) as usize).copied())
                        .unwrap_or(StdDuration::from_secs(5));
                    tokio::time::sleep(delay).await;
                    continue;
                }
                break Err(error);
            }
        }
    };

    match result {
        Ok(image) => {
            let file_name = image.file_name();
            let embed = CreateEmbed::new()
                .title("🖼️ Gambar Sudah Jadi")
                .description(format!("**Prompt:** {prompt}"))
                .color(COLOR_INFO)
                .image(format!("attachment://{file_name}"))
                .footer(
                    CreateEmbedFooter::new(format!(
                        "Powered by Jarvis Image API • Atas permintaan {}",
                        ctx.author().name
                    ))
                    .icon_url(ctx.author().face()),
                );

            let attachment = CreateAttachment::bytes(image.bytes, file_name);
            match ctx
                .send(
                    poise::CreateReply::default()
                        .embed(embed)
                        .attachment(attachment),
                )
                .await
            {
                Ok(_) => {
                    let _ = status_msg.delete(ctx).await;
                }
                Err(error) => {
                    tracing::error!(error = %error, "gagal mengirim gambar ke discord");
                    let embed = err(
                        format!(
                            "⚠️ Gambarnya sudah jadi, tapi gagal saya kirim, {HONORIFIC}. Coba lagi ya."
                        ),
                        None,
                    );
                    let _ = status_msg
                        .edit(ctx, poise::CreateReply::default().embed(embed))
                        .await;
                }
            }
        }
        Err(error) => {
            tracing::warn!(
                error_kind = error.kind(),
                "gambar gagal setelah semua percobaan"
            );
            let embed = err(error.user_message(), None);
            let _ = status_msg
                .edit(ctx, poise::CreateReply::default().embed(embed))
                .await;
        }
    }

    Ok(())
}

#[cfg(test)]
mod image_tests {
    use super::*;

    #[test]
    fn percent_decode_menerjemahkan_escape() {
        assert_eq!(percent_decode("a%20b%2Fc"), "a b/c");
        assert_eq!(percent_decode("100%25"), "100%");
        assert_eq!(percent_decode("tanpa_escape"), "tanpa_escape");
    }

    #[test]
    fn percent_decode_membiarkan_escape_tidak_lengkap() {
        assert_eq!(percent_decode("bad%2"), "bad%2");
        assert_eq!(percent_decode("%ZZ"), "%ZZ");
    }

    #[test]
    fn percent_decode_mengganti_byte_tidak_valid() {
        assert_eq!(percent_decode("%FF"), "\u{FFFD}");
    }

    #[test]
    fn status_dipetakan_ke_varian_yang_tepat() {
        assert!(matches!(
            ImageError::from_status(400, None),
            ImageError::PromptTooLong
        ));
        assert!(matches!(
            ImageError::from_status(401, None),
            ImageError::Unauthorized
        ));
        assert!(matches!(
            ImageError::from_status(403, None),
            ImageError::Unauthorized
        ));
        assert!(matches!(
            ImageError::from_status(429, Some(3)),
            ImageError::RateLimited(Some(3))
        ));
        assert!(matches!(
            ImageError::from_status(503, None),
            ImageError::Upstream
        ));
        assert!(matches!(
            ImageError::from_status(418, None),
            ImageError::BadResponse
        ));
    }

    #[test]
    fn hanya_error_sementara_yang_retryable() {
        assert!(ImageError::Timeout.is_retryable());
        assert!(ImageError::RateLimited(None).is_retryable());
        assert!(ImageError::Upstream.is_retryable());
        assert!(ImageError::Network.is_retryable());

        assert!(!ImageError::NotConfigured.is_retryable());
        assert!(!ImageError::PromptTooLong.is_retryable());
        assert!(!ImageError::Unauthorized.is_retryable());
        assert!(!ImageError::BadResponse.is_retryable());
        assert!(!ImageError::TooLarge.is_retryable());
    }

    #[test]
    fn retry_after_hanya_dipakai_bila_wajar() {
        assert_eq!(
            ImageError::RateLimited(Some(5)).retry_after(),
            Some(StdDuration::from_secs(5))
        );
        assert_eq!(ImageError::RateLimited(Some(30)).retry_after(), None);
        assert_eq!(ImageError::RateLimited(None).retry_after(), None);
        assert_eq!(ImageError::Timeout.retry_after(), None);
    }
}
