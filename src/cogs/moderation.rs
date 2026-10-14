//! Moderation cog: sanctions, channel controls, and warnings.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chrono::{Duration, Utc};
use poise::serenity_prelude as serenity;
use serenity::all::{
    ChannelId, CreateEmbed, EditChannel, EditMember, EditRole, GetMessages, GuildChannel, GuildId,
    Member, MemberParseError, MessageId, PermissionOverwrite, PermissionOverwriteType, Permissions,
    UserId,
};

use crate::helpers::{err, ok, warn, COLOR_ERROR, COLOR_OK, COLOR_WARN};
use crate::{Context, Data, Error};

/// Semua perintah pada cog moderasi.
pub fn commands() -> Vec<poise::Command<Data, Error>> {
    vec![
        ban(),
        unban(),
        kick(),
        timeout(),
        untimeout(),
        purge(),
        warn_member(),
        warnings(),
        clearwarns(),
        slowmode(),
        lock(),
        unlock(),
        mute(),
        unmute(),
    ]
}

fn mod_embed(title: &str, desc: impl Into<String>, color: u32) -> CreateEmbed {
    CreateEmbed::new().title(title).description(desc).color(color)
}

fn failed(message: impl Into<String>) -> CreateEmbed {
    mod_embed("❌ Gagal", message, COLOR_ERROR)
}

fn mention(user_id: UserId) -> String {
    format!("<@{user_id}>")
}

/// Salinan channel guild dari cache, agar tidak menyimpan guard non-`Send`.
fn cached_channel(ctx: &Context<'_>, channel_id: ChannelId) -> Option<GuildChannel> {
    ctx.guild_id()
        .and_then(|guild_id| ctx.serenity_context().cache.guild(guild_id))
        .and_then(|guild| guild.channels.get(&channel_id).cloned())
}

/// Posisi role tertinggi seorang member, padanan `member.top_role`.
fn top_role_position(ctx: &Context<'_>, member: &Member) -> u16 {
    let Some(guild_id) = ctx.guild_id() else {
        return 0;
    };
    let Some(guild) = ctx.serenity_context().cache.guild(guild_id) else {
        return 0;
    };
    member
        .roles
        .iter()
        .filter_map(|role_id| guild.roles.get(role_id))
        .map(|role| role.position)
        .max()
        .unwrap_or(0)
}

/// Cek hierarki role, padanan `_can_moderate`.
async fn can_moderate(ctx: &Context<'_>, target: &Member) -> Result<(), String> {
    let guild_id = ctx
        .guild_id()
        .ok_or_else(|| "❌ Akses Ditolak".to_string())?;
    let author_id = ctx.author().id;
    let bot_id = ctx.serenity_context().cache.current_user().id;
    let owner_id = ctx
        .serenity_context()
        .cache
        .guild(guild_id)
        .map(|guild| guild.owner_id);

    if target.user.id == author_id {
        return Err("❌ Kamu tidak bisa memoderasi diri sendiri.".to_string());
    }
    if owner_id == Some(target.user.id) {
        return Err("❌ Pemilik server tidak bisa dimoderasi.".to_string());
    }

    let target_top = top_role_position(ctx, target);
    let author_top = guild_id
        .member(ctx.http(), author_id)
        .await
        .map(|member| top_role_position(ctx, &member))
        .unwrap_or(0);
    if target_top >= author_top && Some(author_id) != owner_id {
        return Err("❌ Role kamu tidak cukup tinggi untuk memoderasi pengguna ini.".to_string());
    }

    let bot_top = guild_id
        .member(ctx.http(), bot_id)
        .await
        .map(|member| top_role_position(ctx, &member))
        .unwrap_or(0);
    if target_top >= bot_top {
        return Err("❌ Role bot tidak cukup tinggi untuk memoderasi pengguna ini.".to_string());
    }

    Ok(())
}

fn is_status(error: &serenity::Error, status: u16) -> bool {
    match error {
        serenity::Error::Http(serenity::HttpError::UnsuccessfulRequest(response)) => {
            response.status_code.as_u16() == status
        }
        _ => false,
    }
}

// ────────────────────────────────────────────────────────────
// BAN / UNBAN
// ────────────────────────────────────────────────────────────

/// Ban user dari server
#[poise::command(
    prefix_command,
    rename = "ban",
    guild_only,
    required_permissions = "BAN_MEMBERS",
    on_error = "mod_error"
)]
pub async fn ban(
    ctx: Context<'_>,
    member: Member,
    #[rest] reason: Option<String>,
) -> Result<(), Error> {
    if let Err(message) = can_moderate(&ctx, &member).await {
        ctx.send(poise::CreateReply::default().embed(failed(message)))
            .await?;
        return Ok(());
    }

    let reason = reason.unwrap_or_else(|| "Tidak ada alasan".to_string());
    member
        .ban_with_reason(
            ctx.http(),
            0,
            format!("{reason} | Oleh: {}", ctx.author().name),
        )
        .await?;

    let embed = CreateEmbed::new()
        .title("🔨 Member Dibanned")
        .color(COLOR_ERROR)
        .field(
            "User",
            format!("{} (`{}`)", member.user.name, member.user.id),
            false,
        )
        .field("Alasan", reason, false)
        .field("Oleh", mention(ctx.author().id), false)
        .thumbnail(member.face());

    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Unban user dari server
#[poise::command(
    prefix_command,
    rename = "unban",
    guild_only,
    required_permissions = "BAN_MEMBERS",
    on_error = "mod_error"
)]
pub async fn unban(ctx: Context<'_>, user_id: u64) -> Result<(), Error> {
    let guild_id = ctx.guild_id().ok_or_else(|| anyhow::anyhow!("bukan guild"))?;

    let fetched = ctx.http().get_user(UserId::new(user_id)).await;
    match fetched {
        Ok(user) => match guild_id.unban(ctx.http(), user.id).await {
            Ok(()) => {
                let embed = CreateEmbed::new()
                    .title("✅ Member Diunban")
                    .color(COLOR_OK)
                    .field("User", format!("{} (`{}`)", user.name, user.id), false)
                    .field("Oleh", mention(ctx.author().id), false);
                ctx.send(poise::CreateReply::default().embed(embed)).await?;
                Ok(())
            }
            Err(error) if is_status(&error, 404) => {
                ctx.send(poise::CreateReply::default().embed(failed(
                    "Pengguna tidak ditemukan atau tidak ada dalam daftar ban.",
                )))
                .await?;
                Ok(())
            }
            Err(error) => Err(error.into()),
        },
        Err(error) if is_status(&error, 404) => {
            ctx.send(poise::CreateReply::default().embed(failed(
                "Pengguna tidak ditemukan atau tidak ada dalam daftar ban.",
            )))
            .await?;
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

// ────────────────────────────────────────────────────────────
// KICK
// ────────────────────────────────────────────────────────────

/// Kick user dari server
#[poise::command(
    prefix_command,
    rename = "kick",
    guild_only,
    required_permissions = "KICK_MEMBERS",
    on_error = "mod_error"
)]
pub async fn kick(
    ctx: Context<'_>,
    member: Member,
    #[rest] reason: Option<String>,
) -> Result<(), Error> {
    if let Err(message) = can_moderate(&ctx, &member).await {
        ctx.send(poise::CreateReply::default().embed(failed(message)))
            .await?;
        return Ok(());
    }

    let reason = reason.unwrap_or_else(|| "Tidak ada alasan".to_string());
    member
        .kick_with_reason(
            ctx.http(),
            &format!("{reason} | Oleh: {}", ctx.author().name),
        )
        .await?;

    let embed = CreateEmbed::new()
        .title("👢 Member Dikick")
        .color(0xE6_7E_22)
        .field(
            "User",
            format!("{} (`{}`)", member.user.name, member.user.id),
            false,
        )
        .field("Alasan", reason, false)
        .field("Oleh", mention(ctx.author().id), false)
        .thumbnail(member.face());

    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

// ────────────────────────────────────────────────────────────
// TIMEOUT / UNTIMEOUT
// ────────────────────────────────────────────────────────────

/// Timeout user (menit). Contoh: !jarvis timeout @user 10 spam
#[poise::command(
    prefix_command,
    rename = "timeout",
    guild_only,
    required_permissions = "MODERATE_MEMBERS",
    on_error = "mod_error"
)]
pub async fn timeout(
    ctx: Context<'_>,
    member: Member,
    durasi: Option<i64>,
    #[rest] reason: Option<String>,
) -> Result<(), Error> {
    if let Err(message) = can_moderate(&ctx, &member).await {
        ctx.send(poise::CreateReply::default().embed(failed(message)))
            .await?;
        return Ok(());
    }

    let durasi = durasi.unwrap_or(10);
    if !(1..=40_320).contains(&durasi) {
        ctx.send(poise::CreateReply::default().embed(failed(
            "Durasi harus antara 1–40320 menit (maks 28 hari).",
        )))
        .await?;
        return Ok(());
    }

    let reason = reason.unwrap_or_else(|| "Tidak ada alasan".to_string());
    let until = Utc::now() + Duration::minutes(durasi);

    ctx.guild_id()
        .ok_or_else(|| anyhow::anyhow!("bukan guild"))?
        .edit_member(
            ctx.http(),
            member.user.id,
            EditMember::new()
                .disable_communication_until_datetime(until.into())
                .audit_log_reason(&format!("{reason} | Oleh: {}", ctx.author().name)),
        )
        .await?;

    let embed = CreateEmbed::new()
        .title("🔇 Member Di-timeout")
        .color(COLOR_WARN)
        .field(
            "User",
            format!("{} (`{}`)", member.user.name, member.user.id),
            false,
        )
        .field("Durasi", format!("{durasi} menit"), true)
        .field("Berakhir", format!("<t:{}:R>", until.timestamp()), true)
        .field("Alasan", reason, false)
        .field("Oleh", mention(ctx.author().id), false)
        .thumbnail(member.face());

    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Hapus timeout user
#[poise::command(
    prefix_command,
    rename = "untimeout",
    guild_only,
    required_permissions = "MODERATE_MEMBERS",
    on_error = "mod_error"
)]
pub async fn untimeout(ctx: Context<'_>, member: Member) -> Result<(), Error> {
    ctx.guild_id()
        .ok_or_else(|| anyhow::anyhow!("bukan guild"))?
        .edit_member(
            ctx.http(),
            member.user.id,
            EditMember::new().enable_communication(),
        )
        .await?;

    let embed = CreateEmbed::new()
        .title("🔊 Timeout Dihapus")
        .color(COLOR_OK)
        .field(
            "User",
            format!("{} (`{}`)", member.user.name, member.user.id),
            false,
        )
        .field("Oleh", mention(ctx.author().id), false);

    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

// ────────────────────────────────────────────────────────────
// PURGE
// ────────────────────────────────────────────────────────────

/// Hapus pesan. Contoh: !jarvis purge 10
#[poise::command(
    prefix_command,
    rename = "purge",
    aliases("clear"),
    guild_only,
    required_permissions = "MANAGE_MESSAGES",
    on_error = "mod_error"
)]
pub async fn purge(
    ctx: Context<'_>,
    jumlah: i64,
    member: Option<Member>,
) -> Result<(), Error> {
    if !(1..=100).contains(&jumlah) {
        ctx.send(poise::CreateReply::default().embed(failed(
            "Jumlah pesan harus antara 1–100.",
        )))
        .await?;
        return Ok(());
    }

    let Some(invocation) = (match ctx {
        poise::Context::Prefix(prefix) => Some(prefix.msg.clone()),
        _ => None,
    }) else {
        return Ok(());
    };

    let channel_id = ctx.channel_id();
    let _ = channel_id.delete_message(ctx.http(), invocation.id).await;

    let cutoff = Utc::now() - Duration::days(14);

    let to_delete: Vec<MessageId> = if let Some(member) = member {
        let history = channel_id
            .messages(
                ctx.http(),
                GetMessages::new()
                    .limit((jumlah * 10).min(100) as u8)
                    .before(invocation.id),
            )
            .await
            .unwrap_or_default();

        history
            .iter()
            .filter(|message| {
                message.author.id == member.user.id && *message.timestamp > cutoff
            })
            .take(jumlah as usize)
            .map(|message| message.id)
            .collect()
    } else {
        let history = channel_id
            .messages(
                ctx.http(),
                GetMessages::new().limit(jumlah as u8),
            )
            .await
            .unwrap_or_default();

        history
            .iter()
            .filter(|message| *message.timestamp > cutoff)
            .map(|message| message.id)
            .collect()
    };

    let deleted_count = to_delete.len();
    if !to_delete.is_empty() {
        channel_id.delete_messages(ctx.http(), &to_delete).await?;
    }

    let embed = ok(
        format!(
            "🗑️ **{deleted_count} pesan** sudah dihapus oleh {}.",
            mention(ctx.author().id)
        ),
        None,
    );
    let msg = ctx.send(poise::CreateReply::default().embed(embed)).await?;
    tokio::time::sleep(std::time::Duration::from_secs(4)).await;
    let _ = msg.delete(ctx).await;
    Ok(())
}

// ────────────────────────────────────────────────────────────
// WARN (disimpan di memori, reset saat bot restart)
// ────────────────────────────────────────────────────────────

/// Beri peringatan ke user
#[poise::command(
    prefix_command,
    rename = "warn",
    guild_only,
    required_permissions = "KICK_MEMBERS",
    on_error = "mod_error"
)]
pub async fn warn_member(
    ctx: Context<'_>,
    member: Member,
    #[rest] reason: Option<String>,
) -> Result<(), Error> {
    if let Err(message) = can_moderate(&ctx, &member).await {
        ctx.send(poise::CreateReply::default().embed(failed(message)))
            .await?;
        return Ok(());
    }

    let reason = reason.unwrap_or_else(|| "Tidak ada alasan".to_string());
    let guild_id = ctx.guild_id().ok_or_else(|| anyhow::anyhow!("bukan guild"))?;
    let key = (guild_id, member.user.id);

    let count = {
        let mut guard = ctx
            .data()
            .warnings
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let entries = guard.entry(key).or_default();
        entries.push(crate::cogs::moderation::WarnEntry {
            reason: reason.clone(),
            by: ctx.author().name.clone(),
        });
        entries.len()
    };

    let embed = CreateEmbed::new()
        .title("⚠️ Member Diperingatkan")
        .color(COLOR_WARN)
        .field(
            "User",
            format!("{} (`{}`)", member.user.name, member.user.id),
            false,
        )
        .field("Alasan", reason, false)
        .field("Total Warn", format!("**{count}x**"), true)
        .field("Oleh", mention(ctx.author().id), true)
        .thumbnail(member.face());

    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Satu entri peringatan yang tersimpan di memori.
#[derive(Clone)]
pub struct WarnEntry {
    pub reason: String,
    pub by: String,
}

/// Peringatan per (guild, user); padanan `bot._warnings` yang hanya hidup di memori.
pub type Warnings = Arc<Mutex<HashMap<(GuildId, UserId), Vec<WarnEntry>>>>;

/// Lihat daftar warning user
#[poise::command(
    prefix_command,
    rename = "warnings",
    aliases("warnlist"),
    guild_only,
    required_permissions = "KICK_MEMBERS",
    on_error = "mod_error"
)]
pub async fn warnings(ctx: Context<'_>, member: Member) -> Result<(), Error> {
    let guild_id = ctx.guild_id().ok_or_else(|| anyhow::anyhow!("bukan guild"))?;
    let entries = {
        let guard = ctx
            .data()
            .warnings
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        guard
            .get(&(guild_id, member.user.id))
            .cloned()
            .unwrap_or_default()
    };

    if entries.is_empty() {
        let embed = ok(
            format!("✅ {} tidak memiliki peringatan.", mention(member.user.id)),
            None,
        );
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    }

    let mut embed = CreateEmbed::new()
        .title(format!("⚠️ Peringatan — {}", member.user.name))
        .color(COLOR_WARN)
        .thumbnail(member.face());

    for (index, entry) in entries.iter().enumerate() {
        embed = embed.field(
            format!("#{}", index + 1),
            format!("**Alasan:** {}\n**Oleh:** {}", entry.reason, entry.by),
            false,
        );
    }

    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Hapus semua warning user
#[poise::command(
    prefix_command,
    rename = "clearwarns",
    guild_only,
    required_permissions = "ADMINISTRATOR",
    on_error = "mod_error"
)]
pub async fn clearwarns(ctx: Context<'_>, member: Member) -> Result<(), Error> {
    let guild_id = ctx.guild_id().ok_or_else(|| anyhow::anyhow!("bukan guild"))?;
    {
        let mut guard = ctx
            .data()
            .warnings
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        guard.remove(&(guild_id, member.user.id));
    }

    let embed = ok(
        format!(
            "✅ Semua peringatan {} sudah dihapus.",
            mention(member.user.id)
        ),
        None,
    );
    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

// ────────────────────────────────────────────────────────────
// SLOWMODE
// ────────────────────────────────────────────────────────────

/// Set slowmode channel (detik). 0 untuk matikan.
#[poise::command(
    prefix_command,
    rename = "slowmode",
    guild_only,
    required_permissions = "MANAGE_CHANNELS",
    on_error = "mod_error"
)]
pub async fn slowmode(ctx: Context<'_>, detik: i64) -> Result<(), Error> {
    if !(0..=21_600).contains(&detik) {
        ctx.send(poise::CreateReply::default().embed(failed(
            "Slowmode harus antara 0–21600 detik.",
        )))
        .await?;
        return Ok(());
    }

    ctx.channel_id()
        .edit(
            ctx.http(),
            EditChannel::new().rate_limit_per_user(detik as u16),
        )
        .await?;

    let message = if detik == 0 {
        "✅ Slowmode **dimatikan**.".to_string()
    } else {
        format!("✅ Slowmode diatur ke **{detik} detik**.")
    };
    ctx.send(poise::CreateReply::default().embed(ok(message, None)))
        .await?;
    Ok(())
}

// ────────────────────────────────────────────────────────────
// LOCK / UNLOCK
// ────────────────────────────────────────────────────────────

fn channel_name(ctx: &Context<'_>) -> String {
    cached_channel(ctx, ctx.channel_id())
        .map(|channel| channel.name.clone())
        .unwrap_or_default()
}

/// Overwrite channel saat ini, digabung dengan perubahan `send_messages`.
fn overwrites_with_send_messages(
    ctx: &Context<'_>,
    enabled: Option<bool>,
) -> Vec<PermissionOverwrite> {
    let channel_id = ctx.channel_id();
    let everyone = ctx.guild_id().map(|guild_id| guild_id.everyone_role());

    let mut overwrites = cached_channel(ctx, channel_id)
        .map(|channel| channel.permission_overwrites.clone())
        .unwrap_or_default();

    let Some(everyone) = everyone else {
        return overwrites;
    };

    let mut entry = overwrites
        .iter()
        .find(|overwrite| overwrite.kind == PermissionOverwriteType::Role(everyone))
        .cloned()
        .unwrap_or(PermissionOverwrite {
            allow: Permissions::empty(),
            deny: Permissions::empty(),
            kind: PermissionOverwriteType::Role(everyone),
        });

    entry.allow.remove(Permissions::SEND_MESSAGES);
    entry.deny.remove(Permissions::SEND_MESSAGES);
    match enabled {
        Some(true) => {
            entry.allow.insert(Permissions::SEND_MESSAGES);
        }
        Some(false) => {
            entry.deny.insert(Permissions::SEND_MESSAGES);
        }
        None => {
            entry.deny.remove(Permissions::SEND_MESSAGES);
        }
    }

    overwrites.retain(|overwrite| overwrite.kind != PermissionOverwriteType::Role(everyone));
    overwrites.push(entry);
    overwrites
}

/// Kunci channel agar member tidak bisa mengirim pesan
#[poise::command(
    prefix_command,
    rename = "lock",
    guild_only,
    required_permissions = "MANAGE_CHANNELS",
    on_error = "mod_error"
)]
pub async fn lock(ctx: Context<'_>) -> Result<(), Error> {
    let name = channel_name(&ctx);
    let overwrites = overwrites_with_send_messages(&ctx, Some(false));

    ctx.channel_id()
        .edit(ctx.http(), EditChannel::new().permissions(overwrites))
        .await?;

    let embed = err(
        format!(
            "🔒 Channel **{name}** dikunci oleh {}.",
            mention(ctx.author().id)
        ),
        None,
    );
    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Buka kunci channel
#[poise::command(
    prefix_command,
    rename = "unlock",
    guild_only,
    required_permissions = "MANAGE_CHANNELS",
    on_error = "mod_error"
)]
pub async fn unlock(ctx: Context<'_>) -> Result<(), Error> {
    let name = channel_name(&ctx);
    let overwrites = overwrites_with_send_messages(&ctx, None);

    ctx.channel_id()
        .edit(ctx.http(), EditChannel::new().permissions(overwrites))
        .await?;

    let embed = ok(
        format!(
            "🔓 Channel **{name}** dibuka oleh {}.",
            mention(ctx.author().id)
        ),
        None,
    );
    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

// ────────────────────────────────────────────────────────────
// MUTE / UNMUTE
// ────────────────────────────────────────────────────────────

/// Mute user
#[poise::command(
    prefix_command,
    rename = "mute",
    guild_only,
    required_permissions = "MANAGE_ROLES",
    on_error = "mod_error"
)]
pub async fn mute(
    ctx: Context<'_>,
    member: Member,
    #[rest] reason: Option<String>,
) -> Result<(), Error> {
    if let Err(message) = can_moderate(&ctx, &member).await {
        ctx.send(poise::CreateReply::default().embed(failed(message)))
            .await?;
        return Ok(());
    }

    let reason = reason.unwrap_or_else(|| "Tidak ada alasan".to_string());
    let guild_id = ctx.guild_id().ok_or_else(|| anyhow::anyhow!("bukan guild"))?;

    let existing = ctx
        .serenity_context()
        .cache
        .guild(guild_id)
        .and_then(|guild| {
            guild
                .roles
                .values()
                .find(|role| role.name == "Muted")
                .map(|role| role.id)
        });

    let muted_role = match existing {
        Some(role_id) => role_id,
        None => {
            let role = guild_id
                .create_role(
                    ctx.http(),
                    EditRole::new()
                        .name("Muted")
                        .permissions(Permissions::empty()),
                )
                .await?;

            ctx.send(poise::CreateReply::default().embed(warn(
                "Menyiapkan role Muted, tunggu sebentar ya",
                None,
            )))
            .await?;

            let channels: Vec<ChannelId> = ctx
                .serenity_context()
                .cache
                .guild(guild_id)
                .map(|guild| guild.channels.keys().copied().collect())
                .unwrap_or_default();

            for chunk in channels.chunks(5) {
                for channel_id in chunk {
                    let mut overwrites = cached_channel(&ctx, *channel_id)
                        .map(|channel| channel.permission_overwrites.clone())
                        .unwrap_or_default();
                    overwrites.retain(|overwrite| {
                        overwrite.kind != PermissionOverwriteType::Role(role.id)
                    });
                    overwrites.push(PermissionOverwrite {
                        allow: Permissions::empty(),
                        deny: Permissions::SEND_MESSAGES,
                        kind: PermissionOverwriteType::Role(role.id),
                    });
                    let _ = channel_id
                        .edit(ctx.http(), EditChannel::new().permissions(overwrites))
                        .await;
                }
            }

            role.id
        }
    };

    member.add_role(ctx.http(), muted_role).await?;

    let embed = CreateEmbed::new()
        .title("🔇 Member Dimute")
        .color(COLOR_WARN)
        .field(
            "User",
            format!("{} (`{}`)", member.user.name, member.user.id),
            false,
        )
        .field("Alasan", reason, false)
        .field("Oleh", mention(ctx.author().id), false)
        .thumbnail(member.face());

    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Unmute user
#[poise::command(
    prefix_command,
    rename = "unmute",
    guild_only,
    required_permissions = "MANAGE_ROLES",
    on_error = "mod_error"
)]
pub async fn unmute(ctx: Context<'_>, member: Member) -> Result<(), Error> {
    let guild_id = ctx.guild_id().ok_or_else(|| anyhow::anyhow!("bukan guild"))?;

    let muted_role = ctx
        .serenity_context()
        .cache
        .guild(guild_id)
        .and_then(|guild| {
            guild
                .roles
                .values()
                .find(|role| role.name == "Muted")
                .map(|role| role.id)
        });

    match muted_role {
        Some(role_id) if member.roles.contains(&role_id) => {
            member.remove_role(ctx.http(), role_id).await?;
            let embed = CreateEmbed::new()
                .title("🔊 Member Diunmute")
                .color(COLOR_OK)
                .field(
                    "User",
                    format!("{} (`{}`)", member.user.name, member.user.id),
                    false,
                )
                .field("Oleh", mention(ctx.author().id), false);
            ctx.send(poise::CreateReply::default().embed(embed)).await?;
        }
        _ => {
            ctx.send(poise::CreateReply::default().embed(failed(
                "Pengguna ini tidak sedang dimute.",
            )))
            .await?;
        }
    }

    Ok(())
}

// ────────────────────────────────────────────────────────────
// ERROR HANDLER — padanan `mod_error`
// ────────────────────────────────────────────────────────────

fn missing_param_error(ctx: &Context<'_>) -> String {
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

async fn mod_reply(ctx: Context<'_>, embed: CreateEmbed) {
    if let Err(error) = ctx.send(poise::CreateReply::default().embed(embed)).await {
        println!("[Mod] Gagal mengirim pesan: {error}");
    }
}

/// Handler error khusus perintah moderasi.
async fn mod_error(error: poise::FrameworkError<'_, Data, Error>) {
    match error {
        poise::FrameworkError::MissingUserPermissions {
            missing_permissions,
            ctx,
            ..
        } => {
            let names = missing_permissions
                .map(|permissions| permissions.get_permission_names().join("`, `"))
                .unwrap_or_default();
            mod_reply(
                ctx,
                mod_embed(
                    "❌ Akses Ditolak",
                    format!("Kamu tidak memiliki permission: `{names}`"),
                    COLOR_ERROR,
                ),
            )
            .await;
        }
        poise::FrameworkError::MissingBotPermissions {
            missing_permissions,
            ctx,
            ..
        } => {
            let names = missing_permissions.get_permission_names().join("`, `");
            mod_reply(
                ctx,
                mod_embed(
                    "❌ Bot Kurang Permission",
                    format!("Bot tidak punya permission: `{names}`"),
                    COLOR_ERROR,
                ),
            )
            .await;
        }
        poise::FrameworkError::ArgumentParse {
            error, input, ctx, ..
        } => {
            if error.is::<MemberParseError>() {
                mod_reply(
                    ctx,
                    mod_embed("❌ Gagal", "Member tidak ditemukan.", COLOR_ERROR),
                )
                .await;
            } else if input.is_none() && error.is::<poise::TooFewArguments>() {
                mod_reply(
                    ctx,
                    mod_embed(
                        "❌ Argumen Kurang",
                        format!(
                            "Argumen `{}` masih diperlukan. Ketik `!jarvis` untuk bantuan.",
                            missing_param_error(&ctx)
                        ),
                        COLOR_ERROR,
                    ),
                )
                .await;
            } else {
                mod_reply(
                    ctx,
                    mod_embed(
                        "❌ Argumen Salah",
                        "Pastikan format perintah sudah benar.",
                        COLOR_ERROR,
                    ),
                )
                .await;
            }
        }
        poise::FrameworkError::Command { error, ctx, .. } => {
            if let Some(serenity_error) = error.downcast_ref::<serenity::Error>() {
                if is_status(serenity_error, 403) {
                    mod_reply(
                        ctx,
                        mod_embed(
                            "❌ Akses Ditolak",
                            "Bot tidak memiliki izin untuk melakukan aksi ini. Pastikan posisi role bot berada di atas target.",
                            COLOR_ERROR,
                        ),
                    )
                    .await;
                    return;
                }
            }
            println!("[Mod] Error: {error}");
            mod_reply(
                ctx,
                mod_embed(
                    "⚠️ Error",
                    "Ada yang tidak beres di sisi saya. Coba lagi sebentar lagi ya.",
                    COLOR_ERROR,
                ),
            )
            .await;
        }
        other => {
            println!("[Mod] Error: {other}");
        }
    }
}
