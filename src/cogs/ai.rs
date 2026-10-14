//! AI cog: chat, web search, PDF summarization, and image generation.

use std::net::IpAddr;
use std::time::Duration as StdDuration;

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
                println!("[AI] ✅ Jarvis Custom API: {url}");
                true
            }
            (Some(_), None) => {
                println!(
                    "[AI] ⚠️  JARVIS_API_URL diatur tetapi JARVIS_API_SECRET kosong. Custom API dinonaktifkan."
                );
                false
            }
            (None, _) => {
                if groq_api_key.is_some() {
                    println!("[AI] ✅ Groq API configured (direct).");
                    true
                } else {
                    println!(
                        "[AI] ⚠️  No AI backend configured (set JARVIS_API_URL or GROQ_API_KEY)"
                    );
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
                ("openai/gpt-oss-120b".to_string(), "GPT-OSS 120B".to_string()),
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
        println!("[AI] Trying Groq model: {model_id}");
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
                                println!("[AI] {model_id} failed: {text}");
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
                        println!("[AI] {model_id} failed: {error}");
                        last_error = Some(error.to_string());
                    }
                }
                let _ = status;
            }
            Err(error) => {
                let text = error.to_string();
                println!("[AI] {model_id} failed: {text}");
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

const NO_BACKEND: &str = "❌ Backend AI belum aktif. Set `JARVIS_API_URL` + `JARVIS_API_SECRET`, atau `GROQ_API_KEY`.";

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
            println!("[AI] Error: {error}");
            ctx.send(poise::CreateReply::default().embed(err(
                friendly_error(&error),
                None,
            )))
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
                answer = format!("Maaf, saya belum menemukan info yang relevan untuk itu, {HONORIFIC}.");
            }
            send_response(ctx, &answer, &used_model).await?;
        }
        Err(error) => {
            println!("[AI] Search error: {error}");
            ctx.send(poise::CreateReply::default().embed(err(
                friendly_error(&error),
                None,
            )))
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
                || v6.to_ipv4_mapped()
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
            ctx.send(poise::CreateReply::default().embed(err(
                "❌ File yang dilampirkan bukan PDF.",
                None,
            )))
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
            println!("[AI] Summarize error: {error}");
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
            println!("[AI] Summarize error: {error}");
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
                None => url
                    .rsplit('/')
                    .next()
                    .unwrap_or_default()
                    .to_string(),
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

    println!("[AI] Summarize error: {error}");
    set_status(ctx, &status_msg, &friendly_error(&error), COLOR_ERROR).await?;
    Ok(())
}

// ────────────────────────────────────────────────────────────
// Gambar AI
// ────────────────────────────────────────────────────────────

async fn generate_image(data: &Data, prompt: &str) -> Result<Vec<u8>, String> {
    let url = data
        .ai
        .custom_image_url
        .as_ref()
        .ok_or_else(|| "Image API belum dikonfigurasi.".to_string())?;
    let api_key = data.ai.custom_image_key.clone().unwrap_or_default();

    let response = data
        .http
        .post(url)
        .header("Authorization", format!("Bearer {api_key}"))
        .json(&json!({ "prompt": prompt }))
        .timeout(StdDuration::from_secs(60))
        .send()
        .await
        .map_err(|error| error.to_string())?;

    if response.status().as_u16() != 200 {
        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        let preview: String = text.chars().take(150).collect();
        return Err(format!("Image API HTTP {status}: {preview}"));
    }

    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_lowercase();

    if !content_type.starts_with("image/") {
        let text = response.text().await.unwrap_or_default();
        let preview: String = text.chars().take(150).collect();
        return Err(format!(
            "Respons bukan gambar (Content-Type: {content_type}): {preview}"
        ));
    }

    response
        .bytes()
        .await
        .map(|bytes| bytes.to_vec())
        .map_err(|error| error.to_string())
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
        ctx.send(poise::CreateReply::default().embed(err(
            "❌ **Image API** belum dikonfigurasi di `.env`.",
            None,
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

    match generate_image(ctx.data(), &prompt).await {
        Ok(bytes) => {
            let _ = status_msg.delete(ctx).await;

            let embed = CreateEmbed::new()
                .title("🖼️ Gambar Sudah Jadi")
                .description(format!("**Prompt:** {prompt}"))
                .color(COLOR_INFO)
                .image("attachment://jarvis_image.png")
                .footer(
                    CreateEmbedFooter::new(format!(
                        "Powered by Jarvis Image API • Atas permintaan {}",
                        ctx.author().name
                    ))
                    .icon_url(ctx.author().face()),
                );

            let attachment = CreateAttachment::bytes(bytes, "jarvis_image.png");
            ctx.send(poise::CreateReply::default().embed(embed).attachment(attachment))
                .await?;
        }
        Err(error) => {
            println!("[AI] Image generation failed: {error}");
            let message = if error.trim().is_empty() {
                "Exception".to_string()
            } else {
                error
            };
            let embed = err(format!("⚠️ Maaf, gambar gagal dibuat: `{message}`"), None);
            status_msg
                .edit(ctx, poise::CreateReply::default().embed(embed))
                .await?;
        }
    }

    Ok(())
}
