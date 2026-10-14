//! Utility cog: bot info, latency, uptime, and server/user details.

use chrono::Utc;
use poise::serenity_prelude as serenity;
use serenity::all::{
    ApplicationId, ChannelType, Colour, CreateBotAuthParameters, CreateEmbed, CreateEmbedFooter,
    Member, Permissions, Role, Scope,
};

use crate::config::{COGS, DISCORD_LIBRARY_VERSION};
use crate::helpers::{COLOR_DANGER, COLOR_INFO, COLOR_OK, COLOR_WARN};
use crate::{Context, Data, Error};

/// Semua perintah pada cog utilitas.
///
/// `help_text` poise diisi lewat doc comment mengikuti docstring Python.
pub fn commands() -> Vec<poise::Command<Data, Error>> {
    vec![
        ping(),
        list_cogs(),
        info(),
        help(),
        invite(),
        uptime(),
        stats(),
        userinfo(),
        serverinfo(),
    ]
}

fn footer(ctx: &Context<'_>) -> CreateEmbedFooter {
    CreateEmbedFooter::new(format!("Atas permintaan {}", ctx.author().name))
        .icon_url(ctx.author().face())
}

/// Cek latensi bot
#[poise::command(prefix_command, rename = "ping")]
pub async fn ping(ctx: Context<'_>) -> Result<(), Error> {
    let ws_latency = (ctx.ping().await.as_secs_f64() * 1000.0).round() as i64;

    let start = std::time::Instant::now();
    let msg = ctx.say("🏓 Mengukur latensi...").await?;
    let roundtrip = (start.elapsed().as_secs_f64() * 1000.0).round() as i64;

    let (color, status) = if roundtrip < 100 {
        (COLOR_OK, "🟢 Sangat baik")
    } else if roundtrip < 200 {
        (COLOR_WARN, "🟡 Baik")
    } else if roundtrip < 400 {
        (0xE6_7E_22, "🟠 Sedang")
    } else {
        (COLOR_DANGER, "🔴 Lambat")
    };

    let embed = CreateEmbed::new()
        .title("🏓 Semua sistem responsif")
        .color(color)
        .field("📡 WebSocket", format!("`{ws_latency}ms`"), true)
        .field("↩️ Round-trip", format!("`{roundtrip}ms`"), true)
        .field("📊 Status", status, true)
        .footer(footer(&ctx));

    msg.edit(ctx, poise::CreateReply::default().content("").embed(embed))
        .await?;

    Ok(())
}

/// Tampilkan cogs yang sedang aktif
#[poise::command(prefix_command, rename = "cogs")]
pub async fn list_cogs(ctx: Context<'_>) -> Result<(), Error> {
    let loaded = if COGS.is_empty() {
        "Belum ada".to_string()
    } else {
        COGS.join(", ")
    };

    let embed = CreateEmbed::new()
        .title("📦 Modul Aktif (Cogs)")
        .description(format!("**{loaded}**"))
        .color(COLOR_INFO)
        .footer(footer(&ctx));

    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Lihat info bot
#[poise::command(prefix_command, rename = "info")]
pub async fn info(ctx: Context<'_>) -> Result<(), Error> {
    let bot = ctx.serenity_context().cache.current_user().clone();

    let embed = CreateEmbed::new()
        .title("🤖 Jarvis Bot")
        .description("Asisten serbaguna buat berbagai kebutuhan server kamu.")
        .color(COLOR_INFO)
        .thumbnail(bot.face())
        .field(
            "✨ Fitur Utama",
            "✅ Responsif dan sigap\n✅ Mudah digunakan lewat prefix `!jarvis` / `!j`\n",
            false,
        )
        .field(
            "📌 Informasi",
            format!(
                "**ID:** {}\n**Dibuat:** {}\n**Created By:** @nandhitooo\n**Prefix:** `!jarvis` atau `!j`\n",
                bot.id,
                bot.created_at().format("%Y-%m-%d %H:%M:%S")
            ),
            false,
        );

    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Tampilkan menu bantuan
#[poise::command(prefix_command, rename = "help")]
pub async fn help(ctx: Context<'_>) -> Result<(), Error> {
    crate::send_help_menu(&ctx).await
}

/// Dapatkan link invite bot
#[poise::command(prefix_command, rename = "invite")]
pub async fn invite(ctx: Context<'_>) -> Result<(), Error> {
    let bot_id = ctx.serenity_context().cache.current_user().id;

    let perms = Permissions::VIEW_CHANNEL
        | Permissions::SEND_MESSAGES
        | Permissions::EMBED_LINKS
        | Permissions::ATTACH_FILES
        | Permissions::READ_MESSAGE_HISTORY
        | Permissions::CONNECT
        | Permissions::SPEAK
        | Permissions::MANAGE_MESSAGES
        | Permissions::KICK_MEMBERS
        | Permissions::BAN_MEMBERS
        | Permissions::MODERATE_MEMBERS
        | Permissions::MANAGE_ROLES
        | Permissions::MANAGE_CHANNELS;

    let invite_link = CreateBotAuthParameters::new()
        .client_id(ApplicationId::from(bot_id.get()))
        .permissions(perms)
        .scopes(&[Scope::Bot, Scope::ApplicationsCommands])
        .build();

    let embed = CreateEmbed::new()
        .title("📨 Invite Jarvis")
        .description(format!(
            "Klik [di sini]({invite_link}) buat mengundang saya ke server kamu."
        ))
        .color(COLOR_INFO)
        .footer(footer(&ctx));

    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Lihat berapa lama bot sudah online
#[poise::command(prefix_command, rename = "uptime")]
pub async fn uptime(ctx: Context<'_>) -> Result<(), Error> {
    let now = Utc::now();
    let delta = now - ctx.data().start_time;

    let total = delta.num_seconds().max(0);
    let (hours, remainder) = (total / 3600, total % 3600);
    let (minutes, seconds) = (remainder / 60, remainder % 60);
    let (days, hours) = (hours / 24, hours % 24);

    let mut parts: Vec<String> = Vec::new();
    if days > 0 {
        parts.push(format!("{days} hari"));
    }
    if hours > 0 {
        parts.push(format!("{hours} jam"));
    }
    if minutes > 0 {
        parts.push(format!("{minutes} menit"));
    }
    if seconds > 0 {
        parts.push(format!("{seconds} detik"));
    }

    let uptime_str = if parts.is_empty() {
        "0 detik".to_string()
    } else {
        parts.join(", ")
    };

    let embed = CreateEmbed::new()
        .title("🕒 Waktu Aktif")
        .description(format!("Saya sudah aktif selama **{uptime_str}**."))
        .color(COLOR_INFO)
        .footer(CreateEmbedFooter::new(format!(
            "Aktif sejak: {}",
            ctx.data().start_time.format("%Y-%m-%d %H:%M:%S UTC")
        )));

    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Statistik penggunaan bot
#[poise::command(prefix_command, rename = "stats")]
pub async fn stats(ctx: Context<'_>) -> Result<(), Error> {
    let cache = &ctx.serenity_context().cache;
    let guild_ids = cache.guilds();
    let guild_count = guild_ids.len();
    let user_count: u64 = guild_ids
        .iter()
        .filter_map(|id| cache.guild(id))
        .map(|guild| guild.member_count)
        .sum();
    let latency = (ctx.ping().await.as_secs_f64() * 1000.0).round() as i64;

    let embed = CreateEmbed::new()
        .title("📊 Statistik Jarvis")
        .color(COLOR_INFO)
        .field("🌐 Servers", format!("`{guild_count}`"), true)
        .field("👥 Users", format!("`{user_count}`"), true)
        .field("⚡ Latency", format!("`{latency}ms`"), true)
        .field(
            "⚙️ serenity",
            format!("`{DISCORD_LIBRARY_VERSION}` (serenity)"),
            false,
        )
        .thumbnail(cache.current_user().face())
        .footer(footer(&ctx));

    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Lihat informasi user
#[poise::command(prefix_command, rename = "userinfo", aliases("user", "whois"))]
pub async fn userinfo(ctx: Context<'_>, member: Option<Member>) -> Result<(), Error> {
    let member = match member {
        Some(member) => member,
        None => match ctx.author_member().await {
            Some(member) => member.into_owned(),
            None => return Err(anyhow::anyhow!("userinfo dipanggil tanpa data member")),
        },
    };

    let guild_id = ctx
        .guild_id()
        .ok_or_else(|| anyhow::anyhow!("userinfo dipanggil di luar server"))?;

    // `GuildRef` dari cache bukan `Send`, jadi seluruh pengambilan data diselesaikan
    // di dalam blok ini sebelum ada `.await`.
    let embed = {
        let guild = ctx.serenity_context().cache.guild(guild_id);

        let mut member_roles: Vec<Role> = match &guild {
            Some(guild) => member
                .roles
                .iter()
                .filter_map(|role_id| guild.roles.get(role_id).cloned())
                .collect(),
            None => Vec::new(),
        };
        // Role @everyone tidak ditampilkan (di discord.py ini adalah roles[0]).
        member_roles.retain(|role| role.id.get() != guild_id.get());
        member_roles.sort_by_key(|role| role.position);

        let roles: Vec<String> = member_roles
            .iter()
            .map(|role| format!("<@&{}>", role.id))
            .collect();
        let mut roles_value = if roles.is_empty() {
            "None".to_string()
        } else {
            roles.join(" ")
        };
        if roles_value.chars().count() > 1024 {
            roles_value = roles_value.chars().take(1021).collect::<String>() + "...";
        }

        let color = member_roles
            .iter()
            .filter(|role| role.colour.0 != 0)
            .max_by_key(|role| role.position)
            .map(|role| role.colour)
            .unwrap_or(Colour::default());

        CreateEmbed::new()
            .title(format!("👤 Info Pengguna — {}", member.user.name))
            .color(color)
            .thumbnail(member.face())
            .field("ID", member.user.id.to_string(), true)
            .field(
                "Nickname",
                member.nick.clone().unwrap_or_else(|| "None".to_string()),
                true,
            )
            .field(
                "Akun Dibuat",
                member.user.created_at().format("%d %b %Y").to_string(),
                true,
            )
            .field(
                "Join Server",
                member
                    .joined_at
                    .map(|joined| joined.format("%d %b %Y").to_string())
                    .unwrap_or_else(|| "Unknown".to_string()),
                true,
            )
            .field(
                format!("Roles ({})", member_roles.len()),
                roles_value,
                false,
            )
            .footer(footer(&ctx))
    };

    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Lihat informasi server
#[poise::command(prefix_command, rename = "serverinfo", aliases("server"))]
pub async fn serverinfo(ctx: Context<'_>) -> Result<(), Error> {
    let guild_id = ctx
        .guild_id()
        .ok_or_else(|| anyhow::anyhow!("serverinfo dipanggil di luar server"))?;

    // `GuildRef` dari cache bukan `Send`, jadi seluruh pengambilan data diselesaikan
    // di dalam blok ini sebelum ada `.await`.
    let embed = {
        let guild = ctx
            .serenity_context()
            .cache
            .guild(guild_id)
            .ok_or_else(|| anyhow::anyhow!("data server tidak ada di cache"))?;

        let text_channels = guild
            .channels
            .values()
            .filter(|channel| matches!(channel.kind, ChannelType::Text))
            .count();
        let voice_channels = guild
            .channels
            .values()
            .filter(|channel| channel.kind == ChannelType::Voice)
            .count();

        let owner = guild
            .members
            .get(&guild.owner_id)
            .map(|member| member.user.name.clone())
            .unwrap_or_else(|| guild.owner_id.to_string());

        let mut embed = CreateEmbed::new()
            .title(format!("🏰 Info Server — {}", guild.name))
            .color(COLOR_INFO)
            .field("Owner", owner, true)
            .field("ID", guild.id.to_string(), true)
            .field(
                "Dibuat Pada",
                guild.id.created_at().format("%d %b %Y").to_string(),
                true,
            )
            .field("Member", format!("👥 {}", guild.member_count), true)
            .field(
                "Channel",
                format!("💬 {text_channels} Text | 🔊 {voice_channels} Voice"),
                true,
            )
            .field("Roles", format!("🛡️ {}", guild.roles.len()), true)
            .footer(footer(&ctx));

        if let Some(icon) = guild.icon_url() {
            embed = embed.thumbnail(icon);
        }

        embed
    };

    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}
