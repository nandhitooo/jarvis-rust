//! Jarvis bot entrypoint: client setup, cog loading, help menu, and global error handling.

mod cogs;
mod config;
mod helpers;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use poise::serenity_prelude as serenity;
use serenity::all::{
    ActivityData, ChannelId, ComponentInteraction, ComponentInteractionDataKind, CreateActionRow,
    CreateEmbed, CreateEmbedAuthor, CreateEmbedFooter, CreateInteractionResponse,
    CreateInteractionResponseMessage, CreateMessage, CreateSelectMenu, CreateSelectMenuKind,
    CreateSelectMenuOption, EditMessage, FullEvent, GatewayIntents, Interaction, Message,
    MessageId, OnlineStatus, ReactionType, UserId,
};
use songbird::SerenityInit;

use cogs::{ai, assistant, moderation, music, utility};
use config::{COGS, HONORIFIC};
use helpers::{err, info, warn, COLOR_INFO};

/// Tipe error yang dipakai seluruh perintah.
pub type Error = anyhow::Error;
/// Konteks perintah poise.
pub type Context<'a> = poise::Context<'a, Data, Error>;

/// State bersama bot (padanan `self` pada class `Jarvis` versi Python).
pub struct Data {
    /// Waktu bot mulai berjalan (`self.start_time`).
    pub start_time: DateTime<Utc>,
    /// Pemilik setiap menu bantuan (`HelpView`), dikunci per pesan.
    pub help_menus: Arc<Mutex<HashMap<MessageId, UserId>>>,
    /// HTTP client bersama untuk panggilan API eksternal.
    pub http: reqwest::Client,
    /// Daftar pengingat aktif (`Assistant.reminders`).
    pub reminders: assistant::Reminders,
    /// Peringatan per (guild, user) yang hanya hidup di memori (`bot._warnings`).
    pub warnings: moderation::Warnings,
    /// Konfigurasi backend AI (`AI.__init__`).
    pub ai: ai::AiState,
    /// State pemutaran musik per guild (`Music.__init__`).
    pub music: Arc<music::MusicState>,
}

const HELP_SELECT_ID: &str = "jarvis_help_category";

// ────────────────────────────────────────────────────────────
// Menu bantuan (HelpView)
// ────────────────────────────────────────────────────────────

fn help_options() -> Vec<CreateSelectMenuOption> {
    vec![
        CreateSelectMenuOption::new("Menu Utama", "Main")
            .emoji(ReactionType::Unicode("🏠".to_string()))
            .description("Kembali ke halaman utama"),
        CreateSelectMenuOption::new("Musik", "Music")
            .emoji(ReactionType::Unicode("🎵".to_string()))
            .description("Perintah pemutaran musik"),
        CreateSelectMenuOption::new("AI & Asisten", "AI")
            .emoji(ReactionType::Unicode("🤖".to_string()))
            .description("Fitur kecerdasan buatan"),
        CreateSelectMenuOption::new("Moderator", "Mod")
            .emoji(ReactionType::Unicode("⚙️".to_string()))
            .description("Perintah moderasi server"),
        CreateSelectMenuOption::new("Utilitas", "Util")
            .emoji(ReactionType::Unicode("🛠️".to_string()))
            .description("Informasi dan alat bantu"),
    ]
}

fn help_components(disabled: bool) -> Vec<CreateActionRow> {
    vec![CreateActionRow::SelectMenu(
        CreateSelectMenu::new(
            HELP_SELECT_ID,
            CreateSelectMenuKind::String {
                options: help_options(),
            },
        )
        .placeholder("Pilih kategori perintah...")
        .disabled(disabled),
    )]
}

fn now_hhmm() -> String {
    match config::timezone() {
        Some(timezone) => Utc::now()
            .with_timezone(&timezone)
            .format("%H:%M")
            .to_string(),
        None => chrono::Local::now().format("%H:%M").to_string(),
    }
}

fn main_embed(author_name: &str, author_avatar: &str, bot_avatar: &str) -> CreateEmbed {
    let now = now_hhmm();
    CreateEmbed::new()
        .title("")
        .description(format!(
            "### 🤖 Jarvis — Asisten AI Pribadi\nHai **{author_name}**! Sekarang pukul {now} — ada yang bisa saya bantu?\n\nPilih kategorinya di bawah ini, {HONORIFIC}."
        ))
        .color(COLOR_INFO)
        .author(CreateEmbedAuthor::new("Jarvis Bot").icon_url(bot_avatar))
        .thumbnail(bot_avatar)
        .field("🎵 Musik", "Kontrol pemutaran & antrian lagu", true)
        .field("🤖 AI & Asisten", "Tanya AI, ringkas PDF, pencarian", true)
        .field("⚙️ Moderator", "Kelola anggota & channel", true)
        .field("🛠️ Utilitas", "Info bot, statistik, waktu aktif", true)
        .footer(
            CreateEmbedFooter::new("Jarvis v2.5  •  Pilih menu di bawah")
                .icon_url(author_avatar),
        )
}

fn music_embed() -> CreateEmbed {
    CreateEmbed::new()
        .title("🎵 Menu Musik")
        .color(COLOR_INFO)
        .field(
            "Kontrol Dasar",
            "`!j join` — Bergabung ke voice channel\n\
             `!j play <tautan/judul>` — Putar lagu\n\
             `!j stop` — Berhenti & keluar\n\
             `!j pause` / `!j resume` — Jeda/Lanjut\n\
             `!j skip` — Lewati lagu\n",
            false,
        )
        .field(
            "Antrian & Info",
            "`!j queue` — Lihat daftar putar\n\
             `!j np` — Lagu yang sedang diputar\n\
             `!j ph` — Riwayat pemutaran\n\
             `!j clearqueue` (alias `cq`) — Hapus antrian\n\
             `!j remove <no>` — Hapus lagu tertentu\n\
             `!j volume <0-100>` — Atur volume\n",
            false,
        )
}

fn ai_embed() -> CreateEmbed {
    CreateEmbed::new()
        .title("🤖 AI & Asisten")
        .color(COLOR_INFO)
        .field(
            "Kecerdasan Buatan",
            "`!j ask <tanya>` — Bercakap dengan Jarvis\n\
             `!j search <query>` — Cari informasi\n\
             `!j image <deskripsi>` — Generate gambar AI\n\
             `!j summarize <pdf>` — Ringkas dokumen\n",
            false,
        )
        .field(
            "Asisten Pribadi",
            "`!j remind <waktu> <tugas>` — Setel pengingat\n\
             `!j timer <waktu>` — Setel timer\n\
             `!j weather <kota>` — Cek cuaca\n\
             `!j calc <ekspresi>` — Kalkulator matematika\n\
             `!j sysinfo` — Cek status sistem host\n\
             `!j model` — Lihat model yang aktif\n",
            false,
        )
}

fn mod_embed() -> CreateEmbed {
    CreateEmbed::new()
        .title("⚙️ Menu Moderator")
        .color(COLOR_INFO)
        .field(
            "Sanksi",
            "`!j kick <user>` | `!j ban <user>`\n\
             `!j timeout <user> <menit>`\n\
             `!j warn <user>` | `!j warnings <user>`\n",
            true,
        )
        .field(
            "Channel",
            "`!j clear <jumlah>` — Hapus pesan\n\
             `!j lock` / `!j unlock` — Kunci channel\n\
             `!j slowmode <detik>` — Set slowmode\n",
            true,
        )
}

fn util_embed() -> CreateEmbed {
    CreateEmbed::new()
        .title("🛠️ Menu Utilitas")
        .color(COLOR_INFO)
        .field(
            "Bot Info",
            "`!j ping` — Cek latensi\n\
             `!j uptime` — Waktu aktif\n\
             `!j stats` — Statistik bot\n\
             `!j info` — Tentang Jarvis\n",
            true,
        )
        .field(
            "Server & User",
            "`!j userinfo <user>`\n\
             `!j serverinfo` — Info server\n\
             `!j invite` — Link invite\n",
            true,
        )
}

fn category_embed(
    category: &str,
    author_name: &str,
    author_avatar: &str,
    bot_avatar: &str,
) -> CreateEmbed {
    match category {
        "Music" => music_embed(),
        "AI" => ai_embed(),
        "Mod" => mod_embed(),
        "Util" => util_embed(),
        _ => main_embed(author_name, author_avatar, bot_avatar),
    }
}

/// Menandai menu sebagai milik `author_id` lalu menonaktifkan komponennya setelah 60 detik.
fn track_help_menu(
    serenity_ctx: &serenity::Context,
    data: &Data,
    channel_id: ChannelId,
    message_id: MessageId,
    author_id: UserId,
) {
    let menus = data.help_menus.clone();
    {
        let mut guard = menus.lock().unwrap_or_else(|error| error.into_inner());
        guard.insert(message_id, author_id);
    }

    let serenity_ctx = serenity_ctx.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        {
            let mut guard = menus.lock().unwrap_or_else(|error| error.into_inner());
            guard.remove(&message_id);
        }
        let _ = channel_id
            .edit_message(
                &serenity_ctx,
                message_id,
                EditMessage::new().components(help_components(true)),
            )
            .await;
    });
}

/// Mengirim menu bantuan lalu menjadwalkan penonaktifan komponen setelah 60 detik.
async fn send_help_message(
    serenity_ctx: &serenity::Context,
    data: &Data,
    channel_id: ChannelId,
    author_id: UserId,
    author_name: &str,
    author_avatar: &str,
) -> Result<(), Error> {
    let bot_avatar = serenity_ctx.cache.current_user().face();
    let embed = main_embed(author_name, author_avatar, &bot_avatar);

    let message = channel_id
        .send_message(
            serenity_ctx,
            CreateMessage::new()
                .embed(embed)
                .components(help_components(false)),
        )
        .await?;

    track_help_menu(serenity_ctx, data, channel_id, message.id, author_id);

    Ok(())
}

/// Kirim menu bantuan dari konteks poise (dipakai perintah `help`).
pub async fn send_help_menu(ctx: &Context<'_>) -> Result<(), Error> {
    let author = ctx.author();
    let member = ctx.author_member().await;
    let author_name = member
        .as_ref()
        .map(|member| member.display_name().to_string())
        .unwrap_or_else(|| author.name.clone());
    let author_avatar = member
        .as_ref()
        .map(|member| member.face())
        .unwrap_or_else(|| author.face());

    let bot_avatar = ctx.serenity_context().cache.current_user().face();
    let embed = main_embed(&author_name, &author_avatar, &bot_avatar);

    // `ctx.send` menangani dua mode sekaligus: pesan biasa untuk prefix dan
    // respons interaksi untuk slash command.
    let reply = ctx
        .send(
            poise::CreateReply::default()
                .embed(embed)
                .components(help_components(false)),
        )
        .await?;

    let message_id = match reply.message().await {
        Ok(message) => message.id,
        Err(error) => {
            tracing::warn!(error = %error, "gagal mengambil pesan menu bantuan");
            return Ok(());
        }
    };

    track_help_menu(
        ctx.serenity_context(),
        ctx.data(),
        ctx.channel_id(),
        message_id,
        author.id,
    );

    Ok(())
}

/// Kirim menu bantuan dari sebuah pesan (dipakai saat prefix dipakai tanpa sub-command).
async fn send_help_from_message(
    serenity_ctx: &serenity::Context,
    data: &Data,
    msg: &Message,
) -> Result<(), Error> {
    let author_name = msg
        .member
        .as_ref()
        .and_then(|member| {
            member.user.as_ref().map(|user| {
                member
                    .nick
                    .clone()
                    .or_else(|| user.global_name.clone())
                    .unwrap_or_else(|| user.name.clone())
            })
        })
        .unwrap_or_else(|| msg.author.name.clone());

    send_help_message(
        serenity_ctx,
        data,
        msg.channel_id,
        msg.author.id,
        &author_name,
        &msg.author.face(),
    )
    .await
}

async fn handle_component(
    serenity_ctx: &serenity::Context,
    component: &ComponentInteraction,
    data: &Data,
) -> Result<(), Error> {
    if component.data.custom_id != HELP_SELECT_ID {
        return Ok(());
    }

    let owner = {
        let guard = data
            .help_menus
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        guard.get(&component.message.id).copied()
    };
    let Some(owner) = owner else {
        return Ok(());
    };

    if component.user.id != owner {
        component
            .create_response(
                serenity_ctx,
                CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new()
                        .content("Hanya pemanggil menu ini yang bisa memakainya.")
                        .ephemeral(true),
                ),
            )
            .await?;
        return Ok(());
    }

    let category = match &component.data.kind {
        ComponentInteractionDataKind::StringSelect { values } => values
            .first()
            .cloned()
            .unwrap_or_else(|| "Main".to_string()),
        _ => "Main".to_string(),
    };

    let bot_avatar = serenity_ctx.cache.current_user().face();
    let author_name = component
        .member
        .as_ref()
        .map(|member| member.display_name().to_string())
        .unwrap_or_else(|| component.user.name.clone());
    let author_avatar = component
        .member
        .as_ref()
        .map(|member| member.face())
        .unwrap_or_else(|| component.user.face());

    let embed = category_embed(&category, &author_name, &author_avatar, &bot_avatar);

    component
        .create_response(
            serenity_ctx,
            CreateInteractionResponse::UpdateMessage(
                CreateInteractionResponseMessage::new()
                    .embed(embed)
                    .components(help_components(false)),
            ),
        )
        .await?;

    Ok(())
}

// ────────────────────────────────────────────────────────────
// Saran perintah (saat pengguna salah ketik nama perintah)
// ────────────────────────────────────────────────────────────

/// Batas minimal kemiripan supaya sebuah perintah layak disarankan.
const MIN_SIMILARITY: f64 = 0.5;
/// Jumlah saran yang ditampilkan sekaligus.
const MAX_SUGGESTIONS: usize = 5;

/// Ringkasan satu perintah: nama, alias, dan deskripsi singkatnya.
struct CommandSummary {
    name: String,
    aliases: Vec<String>,
    description: Option<String>,
}

/// Jarak edit Levenshtein (jumlah sisip, hapus, atau ganti karakter).
fn levenshtein(left: &str, right: &str) -> usize {
    let left: Vec<char> = left.chars().collect();
    let right: Vec<char> = right.chars().collect();
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    let mut current = vec![0usize; right.len() + 1];

    for (i, left_char) in left.iter().enumerate() {
        current[0] = i + 1;
        for (j, right_char) in right.iter().enumerate() {
            let substitution = previous[j] + usize::from(left_char != right_char);
            current[j + 1] = substitution.min(previous[j + 1] + 1).min(current[j] + 1);
        }
        std::mem::swap(&mut previous, &mut current);
    }

    previous[right.len()]
}

/// Skor kemiripan 0.0-1.0 antara teks yang diketik dan nama perintah.
fn similarity(typed: &str, candidate: &str) -> f64 {
    if typed.is_empty() || candidate.is_empty() {
        return 0.0;
    }
    if typed == candidate {
        return 1.0;
    }
    if candidate.starts_with(typed) {
        return 0.95;
    }
    if candidate.contains(typed) {
        return 0.8;
    }

    let longest = typed.chars().count().max(candidate.chars().count()) as f64;
    (1.0 - levenshtein(typed, candidate) as f64 / longest).max(0.0)
}

/// Kata pertama setelah prefix pada isi pesan (`!j play x` -> `play`).
fn typed_command(content: &str) -> &str {
    let trimmed = content.trim();
    let rest = ["!jarvis", "!j"]
        .iter()
        .find(|prefix| {
            trimmed
                .get(..prefix.len())
                .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
        })
        .map(|prefix| &trimmed[prefix.len()..])
        .unwrap_or(trimmed);

    rest.split_whitespace().next().unwrap_or("")
}

/// Perintah yang paling mirip dengan yang diketik, urut dari yang terdekat.
fn suggest_commands<'a>(
    candidates: &'a [CommandSummary],
    typed: &str,
    limit: usize,
) -> Vec<&'a CommandSummary> {
    if typed.is_empty() {
        return Vec::new();
    }

    let mut scored: Vec<(&CommandSummary, f64)> = candidates
        .iter()
        .filter_map(|candidate| {
            let score = std::iter::once(&candidate.name)
                .chain(candidate.aliases.iter())
                .map(|word| similarity(typed, word))
                .fold(0.0_f64, f64::max);
            (score >= MIN_SIMILARITY).then_some((candidate, score))
        })
        .collect();

    scored.sort_by(|left, right| {
        right
            .1
            .partial_cmp(&left.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.0.name.cmp(&right.0.name))
    });
    scored.truncate(limit);

    scored.into_iter().map(|(candidate, _)| candidate).collect()
}

/// Salinan ringkas seluruh perintah yang terdaftar di framework.
fn command_summaries(framework: &poise::FrameworkContext<'_, Data, Error>) -> Vec<CommandSummary> {
    framework
        .options
        .commands
        .iter()
        .map(|command| CommandSummary {
            name: command.name.clone(),
            aliases: command.aliases.clone(),
            description: command.description.clone(),
        })
        .collect()
}

/// Judul dan isi pesan saran; dipisah dari embed supaya bisa diuji langsung.
fn suggestion_text(typed: &str, suggestions: &[&CommandSummary]) -> (String, String) {
    if suggestions.is_empty() {
        return (
            "❌ Perintah Tidak Dikenal".to_string(),
            format!(
                "Perintah **`{typed}`** tidak saya kenal. Ketik `!jarvis` untuk lihat daftar perintah."
            ),
        );
    }

    let mut lines = String::new();
    for suggestion in suggestions {
        let description = suggestion
            .description
            .as_deref()
            .and_then(|text| text.lines().next())
            .unwrap_or("")
            .trim();
        if description.is_empty() {
            lines.push_str(&format!("`!j {}`\n", suggestion.name));
        } else {
            lines.push_str(&format!("`!j {}` — {description}\n", suggestion.name));
        }
    }

    (
        "🔎 Mungkin Maksudmu".to_string(),
        format!("Perintah **`{typed}`** tidak ada. Mungkin maksud kamu:\n\n{lines}"),
    )
}

/// Embed berisi saran perintah, atau pesan "tidak dikenal" kalau tidak ada yang mirip.
fn suggestion_embed(typed: &str, suggestions: &[&CommandSummary]) -> CreateEmbed {
    let (title, text) = suggestion_text(typed, suggestions);
    if suggestions.is_empty() {
        err(text, Some(&title))
    } else {
        info(text, Some(&title))
    }
}

// ────────────────────────────────────────────────────────────
// Global error handler
// ────────────────────────────────────────────────────────────

async fn reply_embed(ctx: Context<'_>, embed: CreateEmbed) {
    if let Err(error) = ctx.send(poise::CreateReply::default().embed(embed)).await {
        tracing::error!(error = %error, "gagal mengirim embed error");
    }
}

/// Menentukan nama parameter yang belum diisi, meniru `error.param.name` discord.py.
fn missing_param_name(ctx: &Context<'_>) -> String {
    let params = &ctx.command().parameters;
    if let poise::Context::Prefix(prefix) = ctx {
        let provided = prefix.args.split_whitespace().count();
        if let Some(param) = params.get(provided) {
            return param.name.clone();
        }
    }
    params
        .iter()
        .find(|param| param.required)
        .or_else(|| params.last())
        .map(|param| param.name.clone())
        .unwrap_or_default()
}

async fn on_error(error: poise::FrameworkError<'_, Data, Error>) {
    match error {
        poise::FrameworkError::UnknownCommand {
            msg,
            ctx,
            framework,
            ..
        } => {
            let content = msg.content.trim().to_ascii_lowercase();
            if content == "!jarvis" || content == "!j" {
                if let Err(error) = send_help_from_message(ctx, framework.user_data, msg).await {
                    tracing::error!(error = %error, "gagal mengirim menu bantuan");
                }
                return;
            }

            // Salah ketik nama perintah: tawarkan perintah yang paling mirip.
            let typed = typed_command(&msg.content);
            if typed.is_empty() {
                return;
            }

            let summaries = command_summaries(&framework);
            let suggestions = suggest_commands(&summaries, typed, MAX_SUGGESTIONS);
            tracing::debug!(
                typed = %typed,
                user_id = %msg.author.id,
                suggestions = suggestions.len(),
                "perintah tidak dikenal, mengirim saran"
            );

            let embed = suggestion_embed(typed, &suggestions);
            if let Err(error) = msg
                .channel_id
                .send_message(
                    ctx,
                    CreateMessage::new().embed(embed).reference_message(msg),
                )
                .await
            {
                tracing::error!(error = %error, "gagal mengirim saran perintah");
            }
        }
        poise::FrameworkError::ArgumentParse {
            error, input, ctx, ..
        } => {
            tracing::debug!(
                command = %ctx.command().qualified_name,
                user_id = %ctx.author().id,
                "argumen tidak valid"
            );
            let embed = if input.is_none() && error.is::<poise::TooFewArguments>() {
                err(
                    format!(
                        "Parameter **`{}`** masih dibutuhkan. Ketik `!jarvis` untuk lihat daftar perintah.",
                        missing_param_name(&ctx)
                    ),
                    Some("❌ Argumen Kurang"),
                )
            } else {
                err(
                    "Argumennya kurang pas. Ketik `!jarvis` untuk lihat contohnya.",
                    Some("❌ Format Tidak Valid"),
                )
            };
            reply_embed(ctx, embed).await;
        }
        poise::FrameworkError::CooldownHit {
            remaining_cooldown,
            ctx,
            ..
        } => {
            tracing::debug!(
                command = %ctx.command().qualified_name,
                user_id = %ctx.author().id,
                remaining_ms = remaining_cooldown.as_millis() as u64,
                "cooldown aktif"
            );
            reply_embed(
                ctx,
                warn(
                    format!(
                        "Tunggu {:.0} detik lagi ya.",
                        remaining_cooldown.as_secs_f64()
                    ),
                    Some("⏳ Tunggu Sebentar"),
                ),
            )
            .await;
        }
        poise::FrameworkError::GuildOnly { ctx, .. } => {
            reply_embed(
                ctx,
                err(
                    "Perintah ini cuma bisa dipakai di server.",
                    Some("❌ Hanya di Server"),
                ),
            )
            .await;
        }
        poise::FrameworkError::MissingUserPermissions { ctx, .. }
        | poise::FrameworkError::MissingBotPermissions { ctx, .. } => {
            tracing::warn!(
                command = %ctx.command().qualified_name,
                user_id = %ctx.author().id,
                "permission tidak cukup"
            );
            reply_embed(
                ctx,
                err(
                    "Kamu tidak punya izin untuk perintah ini.",
                    Some("❌ Akses Ditolak"),
                ),
            )
            .await;
        }
        poise::FrameworkError::NotAnOwner { ctx, .. }
        | poise::FrameworkError::CommandCheckFailed { ctx, .. } => {
            reply_embed(
                ctx,
                err(
                    "Kamu tidak punya izin untuk perintah ini.",
                    Some("❌ Akses Ditolak"),
                ),
            )
            .await;
        }
        poise::FrameworkError::Command { error, ctx, .. } => {
            tracing::error!(
                command = %ctx.command().qualified_name,
                user_id = %ctx.author().id,
                error = %error,
                "command gagal"
            );
            reply_embed(
                ctx,
                warn(
                    "Ada yang tidak beres di sisi saya. Coba lagi sebentar lagi ya.",
                    Some("⚠️ Terjadi Kesalahan"),
                ),
            )
            .await;
        }
        _ => {}
    }
}

// ────────────────────────────────────────────────────────────
// Entry point
// ────────────────────────────────────────────────────────────

/// Catat setiap perintah yang diterima sebelum dijalankan.
fn pre_command(ctx: Context<'_>) -> poise::BoxFuture<'_, ()> {
    Box::pin(async move {
        tracing::info!(
            command = %ctx.command().qualified_name,
            user_id = %ctx.author().id,
            guild_id = ?ctx.guild_id(),
            channel_id = %ctx.channel_id(),
            "perintah diterima"
        );
    })
}

fn command_list() -> Vec<poise::Command<Data, Error>> {
    let mut commands = Vec::new();
    commands.extend(utility::commands());
    commands.extend(assistant::commands());
    commands.extend(moderation::commands());
    commands.extend(ai::commands());
    commands.extend(music::commands());
    commands
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let _ = dotenvy::dotenv();

    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        tracing_subscriber::EnvFilter::new(
            "jarvis_rs=info,serenity=warn,songbird=warn,hyper=warn,reqwest=warn",
        )
    });
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let token = std::env::var("DISCORD_TOKEN")
        .ok()
        .filter(|token| !token.is_empty());
    let Some(token) = token else {
        tracing::error!("DISCORD_TOKEN tidak ditemukan di file .env");
        return Ok(());
    };

    let framework = poise::Framework::builder()
        .options(poise::FrameworkOptions {
            commands: command_list(),
            prefix_options: poise::PrefixFrameworkOptions {
                prefix: Some("!jarvis".to_string()),
                additional_prefixes: vec![poise::Prefix::Literal("!j")],
                mention_as_prefix: false,
                ..Default::default()
            },
            on_error: |error| Box::pin(on_error(error)),
            pre_command,
            event_handler: |ctx, event, _framework, data| {
                Box::pin(async move {
                    if let FullEvent::InteractionCreate {
                        interaction: Interaction::Component(component),
                    } = event
                    {
                        if let Err(error) = handle_component(ctx, component, data).await {
                            tracing::error!(error = %error, "interaction gagal");
                        }
                    }
                    Ok(())
                })
            },
            ..Default::default()
        })
        .setup(|ctx, ready, framework| {
            Box::pin(async move {
                for cog in COGS {
                    tracing::info!(cog = %cog.to_lowercase(), "cog dimuat");
                }

                // Daftarkan seluruh perintah sebagai slash command supaya Discord
                // menampilkan autocomplete nama perintah di kolom chat.
                let slash_commands = &framework.options().commands;
                match poise::builtins::register_globally(ctx, slash_commands).await {
                    Ok(()) => tracing::info!(
                        count = slash_commands.len(),
                        "slash command didaftarkan secara global"
                    ),
                    Err(error) => {
                        tracing::error!(error = %error, "gagal mendaftarkan slash command")
                    }
                }

                tracing::info!(user = %ready.user.name, user_id = %ready.user.id, "bot siap");
                tracing::info!(cogs = %COGS.join(", "), "cog aktif");

                ctx.set_presence(
                    Some(ActivityData::listening("!jarvis")),
                    OnlineStatus::Online,
                );

                let reminders: assistant::Reminders = Arc::new(Mutex::new(Vec::new()));
                assistant::spawn_reminder_loop(ctx.http.clone(), reminders.clone());

                Ok(Data {
                    start_time: Utc::now(),
                    help_menus: Arc::new(Mutex::new(HashMap::new())),
                    http: reqwest::Client::new(),
                    reminders,
                    warnings: Arc::new(Mutex::new(HashMap::new())),
                    ai: ai::AiState::from_env(),
                    music: Arc::new(music::MusicState::new(reqwest::Client::new())),
                })
            })
        })
        .build();

    let intents = GatewayIntents::GUILDS
        | GatewayIntents::GUILD_MESSAGES
        | GatewayIntents::DIRECT_MESSAGES
        | GatewayIntents::GUILD_VOICE_STATES
        | GatewayIntents::MESSAGE_CONTENT;

    let mut client = serenity::ClientBuilder::new(&token, intents)
        .framework(framework)
        .register_songbird()
        .await?;

    if let Err(error) = client.start().await {
        let text = error.to_string();
        if text.contains("401") || text.contains("Unauthorized") {
            tracing::error!("token Discord tidak valid, periksa file .env");
        } else {
            tracing::error!(error = %error, "bot crash");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(name: &str, aliases: &[&str], description: &str) -> CommandSummary {
        CommandSummary {
            name: name.to_string(),
            aliases: aliases.iter().map(|alias| alias.to_string()).collect(),
            description: Some(description.to_string()),
        }
    }

    #[test]
    fn levenshtein_counts_single_edits() {
        assert_eq!(levenshtein("", ""), 0);
        assert_eq!(levenshtein("play", "play"), 0);
        assert_eq!(levenshtein("play", "paly"), 2);
        assert_eq!(levenshtein("whois", "whos"), 1);
    }

    #[test]
    fn typed_command_strips_both_prefixes() {
        assert_eq!(typed_command("!j pl"), "pl");
        assert_eq!(typed_command("!jarvis play lagu"), "play");
        assert_eq!(typed_command("  !J pause  sekarang "), "pause");
        assert_eq!(typed_command("!j"), "");
        assert_eq!(typed_command("日é"), "日é");
    }

    #[test]
    fn prefix_match_wins() {
        let commands = vec![
            summary("play", &[], "Putar lagu"),
            summary("pause", &[], "Jeda lagu"),
            summary("stop", &[], "Berhenti"),
        ];

        let names: Vec<&str> = suggest_commands(&commands, "pau", 5)
            .iter()
            .map(|command| command.name.as_str())
            .collect();

        // `pause` paling dekat; `stop` terlalu jauh untuk ikut disarankan.
        assert_eq!(names.first(), Some(&"pause"));
        assert!(!names.contains(&"stop"));
    }

    #[test]
    fn matches_typos_and_aliases() {
        let commands = vec![
            summary("userinfo", &["user", "whois"], "Info user"),
            summary("play", &[], "Putar lagu"),
        ];

        let typos: Vec<&str> = suggest_commands(&commands, "plaay", 5)
            .iter()
            .map(|command| command.name.as_str())
            .collect();
        assert_eq!(typos, vec!["play"]);

        let aliases: Vec<&str> = suggest_commands(&commands, "whos", 5)
            .iter()
            .map(|command| command.name.as_str())
            .collect();
        assert_eq!(aliases, vec!["userinfo"]);
    }

    #[test]
    fn unrelated_input_gets_no_suggestion() {
        let commands = vec![summary("play", &[], "Putar lagu")];
        assert!(suggest_commands(&commands, "zzzz", 5).is_empty());
        assert!(suggest_commands(&commands, "", 5).is_empty());
    }

    #[test]
    fn suggestions_are_capped_and_sorted() {
        let commands: Vec<CommandSummary> = (0..8)
            .map(|index| summary(&format!("play{index}"), &[], "Putar lagu"))
            .collect();

        let suggestions = suggest_commands(&commands, "play", 5);

        assert_eq!(suggestions.len(), 5);
        assert_eq!(suggestions[0].name, "play0");
    }

    #[test]
    fn real_command_registry_produces_suggestions() {
        let summaries: Vec<CommandSummary> = command_list()
            .iter()
            .map(|command| CommandSummary {
                name: command.name.clone(),
                aliases: command.aliases.clone(),
                description: command.description.clone(),
            })
            .collect();

        let names: Vec<&str> = suggest_commands(&summaries, "ply", 5)
            .iter()
            .map(|command| command.name.as_str())
            .collect();
        assert!(names.contains(&"play"), "saran: {names:?}");

        let alias = suggest_commands(&summaries, "whois", 5);
        assert!(alias.iter().any(|command| command.name == "userinfo"));

        // Deskripsi dari doc comment harus terisi supaya saran punya keterangan.
        let play = summaries
            .iter()
            .find(|command| command.name == "play")
            .unwrap();
        assert!(play
            .description
            .as_deref()
            .is_some_and(|text| !text.is_empty()));
    }

    #[test]
    fn suggestion_text_lists_commands_and_falls_back() {
        let commands = vec![summary("play", &[], "Putar lagu\ncontoh: !j play x")];
        let suggestions = suggest_commands(&commands, "pla", 5);
        let (title, text) = suggestion_text("pla", &suggestions);
        assert_eq!(title, "🔎 Mungkin Maksudmu");
        assert_eq!(
            text,
            "Perintah **`pla`** tidak ada. Mungkin maksud kamu:\n\n`!j play` — Putar lagu\n"
        );

        let (title, text) = suggestion_text("zzzz", &[]);
        assert_eq!(title, "❌ Perintah Tidak Dikenal");
        assert!(text.contains("tidak saya kenal"));
    }
}

#[cfg(test)]
mod slash_commands {
    use super::*;

    /// Data yang dikirim ke Discord saat mendaftarkan slash command harus sah:
    /// nama valid, deskripsi terisi, dan opsi wajib berada sebelum opsi opsional.
    #[test]
    fn every_command_registers_as_valid_slash_command() {
        let commands = command_list();
        assert!(!commands.is_empty());

        let mut names: Vec<String> = Vec::new();

        for command in &commands {
            let built = command.create_as_slash_command().unwrap_or_else(|| {
                panic!("`{}` tidak menghasilkan data slash command", command.name)
            });
            let json =
                serde_json::to_value(&built).expect("slash command harus bisa diserialisasi");

            let name = json["name"].as_str().expect("nama slash command");
            assert!(
                (1..=32).contains(&name.chars().count())
                    && name
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "nama slash command tidak valid: `{name}`"
            );
            assert!(
                !names.contains(&name.to_string()),
                "nama duplikat: `{name}`"
            );
            names.push(name.to_string());

            let description = json["description"]
                .as_str()
                .expect("deskripsi slash command");
            assert!(
                !description.trim().is_empty() && description.chars().count() <= 100,
                "deskripsi `{name}` tidak valid: {description:?}"
            );

            let options = json["options"].as_array().expect("daftar opsi");
            assert!(options.len() <= 25, "`{name}` punya terlalu banyak opsi");

            let mut optional_seen = false;
            for option in options {
                let option_name = option["name"].as_str().expect("nama opsi");
                let option_description = option["description"].as_str().expect("deskripsi opsi");
                assert!(option_name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'));
                assert_ne!(
                    option_description, "A slash command parameter",
                    "opsi `{option_name}` di `{name}` belum punya deskripsi"
                );

                let required = option["required"].as_bool().unwrap_or(false);
                assert!(
                    !(required && optional_seen),
                    "opsi wajib `{option_name}` di `{name}` berada setelah opsi opsional"
                );
                optional_seen |= !required;
            }
        }
    }

    #[test]
    fn every_command_keeps_prefix_variant() {
        for command in &command_list() {
            assert!(
                command.prefix_action.is_some(),
                "`{}` kehilangan versi prefix",
                command.name
            );
            assert!(
                command.slash_action.is_some(),
                "`{}` tidak punya versi slash",
                command.name
            );
        }
    }
}
