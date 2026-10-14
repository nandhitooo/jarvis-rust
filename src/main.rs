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
use helpers::{err, warn, COLOR_INFO};

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

    let menus = data.help_menus.clone();
    {
        let mut guard = menus.lock().unwrap_or_else(|error| error.into_inner());
        guard.insert(message.id, author_id);
    }

    let message_id = message.id;
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

    send_help_message(
        ctx.serenity_context(),
        ctx.data(),
        ctx.channel_id(),
        author.id,
        &author_name,
        &author_avatar,
    )
    .await
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
            }
            // Perintah yang tidak dikenal diabaikan.
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
        .setup(|ctx, ready, _framework| {
            Box::pin(async move {
                for cog in COGS {
                    tracing::info!(cog = %cog.to_lowercase(), "cog dimuat");
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
