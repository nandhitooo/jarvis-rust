//! Music cog: voice playback, queue management, and playback history.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration as StdDuration;

use poise::serenity_prelude as serenity;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde_json::Value;
use serenity::all::{ChannelId, CreateEmbed, CreateEmbedFooter, GuildId, UserId};
use songbird::events::{Event, EventContext, EventHandler, TrackEvent};
use songbird::input::{HlsRequest, HttpRequest, Input};
use songbird::tracks::{PlayMode, Track, TrackHandle};
use songbird::Call;
use tokio::sync::Mutex;

use crate::config::HONORIFIC;
use crate::helpers::{err, info, ok, warn, COLOR_DANGER, COLOR_INFO, COLOR_OK};
use crate::{Context, Data, Error};

const SPOTIFY_COLOR: u32 = 0x1D_B9_54;

/// Peminta sebuah lagu (padanan `discord.Member` pada `QueueEntry.requester`).
#[derive(Clone)]
pub struct Requester {
    pub id: UserId,
    pub name: String,
    pub avatar: String,
}

impl Requester {
    pub fn mention(&self) -> String {
        format!("<@{}>", self.id)
    }
}

impl From<&serenity::User> for Requester {
    fn from(user: &serenity::User) -> Self {
        Self {
            id: user.id,
            name: user.name.clone(),
            avatar: user.face(),
        }
    }
}

/// Satu item antrian dengan URL dan judul tampilan.
#[derive(Clone)]
pub struct QueueEntry {
    pub url: String,
    pub title: String,
    pub requester: Option<Requester>,
}

/// Lagu yang sedang diputar (padanan `vc.source` pada discord.py).
#[derive(Clone)]
pub struct CurrentTrack {
    pub title: String,
    pub webpage_url: String,
    pub data: Value,
    pub requester: Option<Requester>,
    pub handle: TrackHandle,
}

struct Inner {
    queues: HashMap<GuildId, VecDeque<QueueEntry>>,
    histories: HashMap<GuildId, VecDeque<String>>,
    volumes: HashMap<GuildId, f32>,
    transitioning: HashSet<GuildId>,
    disconnect_tasks: HashMap<GuildId, tokio::task::JoinHandle<()>>,
    current: HashMap<GuildId, CurrentTrack>,
    channels: HashMap<GuildId, ChannelId>,
}

/// State musik per guild; semuanya in-memory seperti versi Python.
pub struct MusicState {
    pub http: reqwest::Client,
    inner: Mutex<Inner>,
}

impl MusicState {
    pub fn new(http: reqwest::Client) -> Self {
        Self {
            http,
            inner: Mutex::new(Inner {
                queues: HashMap::new(),
                histories: HashMap::new(),
                volumes: HashMap::new(),
                transitioning: HashSet::new(),
                disconnect_tasks: HashMap::new(),
                current: HashMap::new(),
                channels: HashMap::new(),
            }),
        }
    }

    pub async fn queue_snapshot(&self, guild_id: GuildId) -> Vec<QueueEntry> {
        let guard = self.inner.lock().await;
        guard
            .queues
            .get(&guild_id)
            .map(|queue| queue.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub async fn queue_len(&self, guild_id: GuildId) -> usize {
        let guard = self.inner.lock().await;
        guard.queues.get(&guild_id).map(VecDeque::len).unwrap_or(0)
    }

    pub async fn push_back(&self, guild_id: GuildId, entry: QueueEntry) {
        let mut guard = self.inner.lock().await;
        guard.queues.entry(guild_id).or_default().push_back(entry);
    }

    pub async fn push_front(&self, guild_id: GuildId, entry: QueueEntry) {
        let mut guard = self.inner.lock().await;
        guard.queues.entry(guild_id).or_default().push_front(entry);
    }

    pub async fn pop_front(&self, guild_id: GuildId) -> Option<QueueEntry> {
        let mut guard = self.inner.lock().await;
        guard.queues.entry(guild_id).or_default().pop_front()
    }

    pub async fn clear_queue(&self, guild_id: GuildId) {
        let mut guard = self.inner.lock().await;
        if let Some(queue) = guard.queues.get_mut(&guild_id) {
            queue.clear();
        }
    }

    pub async fn remove_at(&self, guild_id: GuildId, index: usize) -> Option<QueueEntry> {
        let mut guard = self.inner.lock().await;
        let queue = guard.queues.entry(guild_id).or_default();
        if index < queue.len() {
            queue.remove(index)
        } else {
            None
        }
    }

    pub async fn history_push(&self, guild_id: GuildId, title: String) {
        let mut guard = self.inner.lock().await;
        let history = guard.histories.entry(guild_id).or_default();
        if history.len() >= 100 {
            history.pop_front();
        }
        history.push_back(title);
    }

    pub async fn history_snapshot(&self, guild_id: GuildId) -> Vec<String> {
        let guard = self.inner.lock().await;
        guard
            .histories
            .get(&guild_id)
            .map(|history| history.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Volume guild (default 0.5), padanan `get_volume`.
    pub async fn volume(&self, guild_id: GuildId) -> f32 {
        let guard = self.inner.lock().await;
        guard.volumes.get(&guild_id).copied().unwrap_or(0.5)
    }

    pub async fn set_volume(&self, guild_id: GuildId, volume: f32) {
        let mut guard = self.inner.lock().await;
        guard.volumes.insert(guild_id, volume);
    }

    pub async fn current(&self, guild_id: GuildId) -> Option<CurrentTrack> {
        let guard = self.inner.lock().await;
        guard.current.get(&guild_id).cloned()
    }

    pub async fn set_current(&self, guild_id: GuildId, track: Option<CurrentTrack>) {
        let mut guard = self.inner.lock().await;
        match track {
            Some(track) => {
                guard.current.insert(guild_id, track);
            }
            None => {
                guard.current.remove(&guild_id);
            }
        }
    }

    /// True bila ada lagu yang sedang diputar atau dijeda.
    pub async fn is_active(&self, guild_id: GuildId) -> bool {
        let guard = self.inner.lock().await;
        match guard.current.get(&guild_id) {
            Some(track) => match track.handle.get_info().await {
                Ok(state) => matches!(state.playing, PlayMode::Play | PlayMode::Pause),
                Err(_) => false,
            },
            None => false,
        }
    }

    pub async fn is_paused(&self, guild_id: GuildId) -> bool {
        let guard = self.inner.lock().await;
        match guard.current.get(&guild_id) {
            Some(track) => matches!(
                track.handle.get_info().await.map(|state| state.playing),
                Ok(PlayMode::Pause)
            ),
            None => false,
        }
    }

    pub async fn is_transitioning(&self, guild_id: GuildId) -> bool {
        let guard = self.inner.lock().await;
        guard.transitioning.contains(&guild_id)
    }

    pub async fn set_transitioning(&self, guild_id: GuildId, value: bool) {
        let mut guard = self.inner.lock().await;
        if value {
            guard.transitioning.insert(guild_id);
        } else {
            guard.transitioning.remove(&guild_id);
        }
    }

    pub async fn set_channel(&self, guild_id: GuildId, channel_id: ChannelId) {
        let mut guard = self.inner.lock().await;
        guard.channels.insert(guild_id, channel_id);
    }

    pub async fn channel(&self, guild_id: GuildId) -> Option<ChannelId> {
        let guard = self.inner.lock().await;
        guard.channels.get(&guild_id).copied()
    }

    /// Membatalkan task auto-disconnect yang sedang menunggu.
    pub async fn cancel_disconnect(&self, guild_id: GuildId) {
        let mut guard = self.inner.lock().await;
        if let Some(task) = guard.disconnect_tasks.remove(&guild_id) {
            task.abort();
        }
    }

    pub async fn schedule_disconnect(&self, guild_id: GuildId, task: tokio::task::JoinHandle<()>) {
        let mut guard = self.inner.lock().await;
        if let Some(previous) = guard.disconnect_tasks.remove(&guild_id) {
            previous.abort();
        }
        guard.disconnect_tasks.insert(guild_id, task);
    }
}

/// Semua perintah pada cog musik.
pub fn commands() -> Vec<poise::Command<Data, Error>> {
    vec![
        join(),
        play(),
        pause(),
        resume(),
        skip(),
        stop(),
        queue(),
        nowplaying(),
        playing_history(),
        volume(),
        clearqueue(),
        remove(),
    ]
}

fn footer(author: Option<&Requester>) -> Option<CreateEmbedFooter> {
    author.map(|author| {
        CreateEmbedFooter::new(format!("Atas permintaan {}", author.name))
            .icon_url(author.avatar.clone())
    })
}

/// Format durasi seperti `format_duration` di Python.
fn format_duration(seconds: Option<f64>) -> String {
    let Some(seconds) = seconds else {
        return "Live / Unknown".to_string();
    };
    if seconds == 0.0 {
        return "Live / Unknown".to_string();
    }
    let total = seconds as i64;
    let minutes = total / 60;
    let seconds = total % 60;
    let hours = minutes / 60;
    let minutes = minutes % 60;
    if hours > 0 {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

fn capitalize(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase(),
        None => String::new(),
    }
}

fn text(data: &Value, key: &str) -> Option<String> {
    data.get(key)
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string())
}

/// Padanan `extract_song_metadata`.
fn extract_song_metadata(data: &Value) -> HashMap<&'static str, String> {
    let mut artist = text(data, "artist")
        .or_else(|| text(data, "creator"))
        .or_else(|| text(data, "track"));

    let mut album = text(data, "album").or_else(|| text(data, "album_artist"));

    if artist.is_none() {
        if let Some(title) = text(data, "title") {
            if let Some((left, _)) = title.split_once(" - ") {
                if left.chars().count() <= 40 {
                    artist = Some(left.trim().to_string());
                }
            }
        }
    }

    let mut metadata = HashMap::new();
    metadata.insert("artist", artist.unwrap_or_else(|| "Unknown".to_string()));
    metadata.insert(
        "album",
        album.take().unwrap_or_else(|| "Unknown".to_string()),
    );
    metadata.insert(
        "uploader",
        text(data, "uploader").unwrap_or_else(|| "Unknown".to_string()),
    );
    metadata.insert(
        "duration",
        format_duration(data.get("duration").and_then(|value| value.as_f64())),
    );
    metadata.insert(
        "extractor",
        capitalize(&text(data, "extractor").unwrap_or_else(|| "Unknown".to_string())),
    );
    metadata
}

fn entry_url(entry: &Value) -> String {
    text(entry, "webpage_url")
        .or_else(|| text(entry, "url"))
        .unwrap_or_else(|| {
            format!(
                "https://www.youtube.com/watch?v={}",
                text(entry, "id").unwrap_or_default()
            )
        })
}

fn build_current_embed(
    data: &Value,
    title: &str,
    webpage_url: &str,
    requester: Option<&Requester>,
) -> CreateEmbed {
    let metadata = extract_song_metadata(data);
    let mut embed = CreateEmbed::new()
        .title("🎶 Sedang Diputar")
        .description(format!("**[{title}]({webpage_url})**"))
        .color(COLOR_INFO)
        .field("Artis", metadata["artist"].clone(), true)
        .field("Album", metadata["album"].clone(), true)
        .field("Uploader", metadata["uploader"].clone(), true)
        .field("Durasi", metadata["duration"].clone(), true)
        .field("Sumber", metadata["extractor"].clone(), true);

    if let Some(thumbnail) = text(data, "thumbnail") {
        embed = embed.thumbnail(thumbnail);
    }
    if let Some(footer) = footer(requester) {
        embed = embed.footer(footer);
    }
    embed
}

fn build_added_embed(
    data: &Value,
    title: &str,
    webpage_url: &str,
    requester: Option<&Requester>,
    queue_len: usize,
) -> CreateEmbed {
    let metadata = extract_song_metadata(data);
    let mut embed = CreateEmbed::new()
        .title("📥 Masuk ke Antrian")
        .description(format!("**[{title}]({webpage_url})**"))
        .color(COLOR_OK)
        .field("Artis", metadata["artist"].clone(), true)
        .field("Album", metadata["album"].clone(), true)
        .field("Uploader", metadata["uploader"].clone(), true)
        .field("Durasi", metadata["duration"].clone(), true)
        .field("Posisi Antrian", format!("#{queue_len}"), true);

    if let Some(thumbnail) = text(data, "thumbnail") {
        embed = embed.thumbnail(thumbnail);
    }
    if let Some(footer) = footer(requester) {
        embed = embed.footer(footer);
    }
    embed
}

// ────────────────────────────────────────────────────────────
// yt-dlp
// ────────────────────────────────────────────────────────────

async fn run_ytdlp(target: &str) -> Result<Value, String> {
    tracing::debug!(target = %target, "yt-dlp mengekstrak info");

    let output = tokio::process::Command::new("yt-dlp")
        .args([
            "-J",
            "--no-warnings",
            "--quiet",
            "--ignore-errors",
            "-f",
            "bestaudio/best",
            "--flat-playlist",
            "--default-search",
            "ytsearch",
            "--socket-timeout",
            "30",
            target,
        ])
        .output()
        .await
        .map_err(|error| format!("yt-dlp tidak bisa dijalankan: {error}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.trim().is_empty() {
        return Err("yt-dlp returned no data — the URL may be invalid or unavailable.".to_string());
    }

    serde_json::from_str::<Value>(&stdout)
        .map_err(|error| format!("yt-dlp download error: {error}"))
}

fn build_input(client: reqwest::Client, data: &Value) -> Result<Input, String> {
    let url = text(data, "url").ok_or_else(|| "URL stream tidak ditemukan.".to_string())?;

    let mut headers = HeaderMap::new();
    if let Some(map) = data.get("http_headers").and_then(|value| value.as_object()) {
        for (key, value) in map {
            if let Some(value) = value.as_str() {
                if let (Ok(name), Ok(value)) = (
                    HeaderName::from_bytes(key.as_bytes()),
                    HeaderValue::from_str(value),
                ) {
                    headers.insert(name, value);
                }
            }
        }
    }

    let protocol = text(data, "protocol").unwrap_or_default();
    let input = if protocol == "m3u8_native" {
        Input::from(HlsRequest::new_with_headers(client, url, headers))
    } else {
        Input::from(HttpRequest {
            client,
            request: url,
            headers,
            content_length: data
                .get("filesize")
                .or_else(|| data.get("filesize_approx"))
                .and_then(|value| value.as_u64()),
        })
    };

    Ok(input)
}

// ────────────────────────────────────────────────────────────
// Spotify → YouTube search
// ────────────────────────────────────────────────────────────

/// Normalisasi URL Spotify regional, mis. `spotify.com/id-id/` → `spotify.com/`.
fn normalize_spotify_url(url: &str) -> String {
    const NEEDLE: &str = "spotify.com/";
    let mut result = String::new();
    let mut rest = url;

    while let Some(position) = rest.find(NEEDLE) {
        result.push_str(&rest[..position + NEEDLE.len()]);
        let after = &rest[position + NEEDLE.len()..];
        let bytes = after.as_bytes();
        let mut consumed = 0;
        if bytes.len() >= 2 && bytes[0].is_ascii_lowercase() && bytes[1].is_ascii_lowercase() {
            consumed = 2;
            if bytes.len() >= 5
                && bytes[2] == b'-'
                && bytes[3].is_ascii_lowercase()
                && bytes[4].is_ascii_lowercase()
            {
                consumed = 5;
            }
            if bytes.get(consumed) == Some(&b'/') {
                consumed += 1;
            } else {
                consumed = 0;
            }
        }
        rest = &after[consumed..];
    }

    result.push_str(rest);
    result
}

async fn spotify_metadata(http: &reqwest::Client, url: &str) -> Option<String> {
    let url = normalize_spotify_url(url);

    let response = http
        .get(&url)
        .header(
            "User-Agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
        )
        .timeout(StdDuration::from_secs(15))
        .send()
        .await
        .ok()?;

    if response.status().as_u16() != 200 {
        return None;
    }

    let body = response.text().await.ok()?;
    let document = scraper::Html::parse_document(&body);

    let title_selector = scraper::Selector::parse("meta[property=\"og:title\"]").ok()?;
    let description_selector =
        scraper::Selector::parse("meta[property=\"og:description\"]").ok()?;

    let title = document
        .select(&title_selector)
        .next()
        .and_then(|element| element.value().attr("content"))
        .map(|content| content.trim().to_string())?;

    if let Some(description) = document
        .select(&description_selector)
        .next()
        .and_then(|element| element.value().attr("content"))
    {
        if description.contains(" · ") {
            let parts: Vec<&str> = description.split(" · ").collect();
            if let Some(artist) = parts.get(1) {
                let artist = artist.replace("Album by ", "");
                return Some(format!("{title} {}", artist.trim()));
            }
        }
    }

    Some(title)
}

/// Bila teks berisi tautan Spotify, ubah menjadi pencarian YouTube.
async fn resolve_search(http: &reqwest::Client, search: &str) -> Result<String, String> {
    if !search.contains("spotify.com") {
        return Ok(search.to_string());
    }

    match spotify_metadata(http, search).await {
        Some(metadata) => {
            tracing::debug!(query = %metadata, "spotify dialihkan ke pencarian youtube");
            Ok(format!("ytsearch:{metadata}"))
        }
        None => Err("Metadata dari tautan Spotify-nya tidak bisa saya ambil.".to_string()),
    }
}

// ────────────────────────────────────────────────────────────
// Playback core
// ────────────────────────────────────────────────────────────

async fn send_embed(
    serenity_ctx: &serenity::Context,
    channel_id: ChannelId,
    embed: CreateEmbed,
) -> Result<(), Error> {
    channel_id
        .send_message(
            serenity_ctx,
            serenity::all::CreateMessage::new().embed(embed),
        )
        .await?;
    Ok(())
}

async fn send_voice_error(
    serenity_ctx: &serenity::Context,
    channel_id: ChannelId,
    embed: CreateEmbed,
) {
    if let Err(error) = send_embed(serenity_ctx, channel_id, embed).await {
        tracing::error!(error = %error, "gagal mengirim pesan voice");
    }
}

/// Pembungkus `Box::pin` supaya rantai `play_next` ⇄ `play_entry` bisa rekursif.
fn play_next_boxed<'a>(
    guild_id: GuildId,
    serenity_ctx: &'a serenity::Context,
    music: &'a Arc<MusicState>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
    Box::pin(play_next(guild_id, serenity_ctx, music))
}

/// Callback setelah setiap lagu selesai (padanan `_after_play`).
async fn play_next(guild_id: GuildId, serenity_ctx: &serenity::Context, music: &Arc<MusicState>) {
    let Some(manager) = songbird::get(serenity_ctx).await else {
        return;
    };
    let Some(call) = manager.get(guild_id) else {
        return;
    };

    if call.lock().await.current_channel().is_none() {
        return;
    }

    match music.pop_front(guild_id).await {
        Some(entry) => {
            music.set_transitioning(guild_id, true).await;
            play_entry(guild_id, serenity_ctx, music, &call, entry).await;
        }
        None => {
            if let Some(channel_id) = music.channel(guild_id).await {
                schedule_disconnect(guild_id, serenity_ctx, music, call.clone(), channel_id).await;
                send_voice_error(
                    serenity_ctx,
                    channel_id,
                    info(
                        "🎵 Antrian habis. Sunyi sekali di sini, kabari saya kalau mau putar lagi.",
                        None,
                    ),
                )
                .await;
            }
        }
    }
}

async fn schedule_disconnect(
    guild_id: GuildId,
    serenity_ctx: &serenity::Context,
    music: &Arc<MusicState>,
    call: Arc<Mutex<Call>>,
    channel_id: ChannelId,
) {
    let serenity_ctx = serenity_ctx.clone();
    let task_music = music.clone();
    let task = tokio::spawn(async move {
        tokio::time::sleep(StdDuration::from_secs(300)).await;

        let is_active = task_music.is_active(guild_id).await;
        let has_queue = task_music.queue_len(guild_id).await > 0;
        if is_active || has_queue {
            return;
        }

        let _ = call.lock().await.leave().await;
        send_voice_error(
            &serenity_ctx,
            channel_id,
            warn(
                format!("💤 Sudah 5 menit sepi, saya keluar dulu ya, {HONORIFIC}."),
                None,
            ),
        )
        .await;
        let guild_name = serenity_ctx
            .cache
            .guild(guild_id)
            .map(|guild| guild.name.clone())
            .unwrap_or_else(|| guild_id.to_string());
        tracing::info!(guild = %guild_name, "auto-disconnect dari voice");
    });

    music.schedule_disconnect(guild_id, task).await;
}

/// Hasil ekstraksi yt-dlp siap diputar (menggantikan objek `YTDLSource`).
struct ResolvedTrack {
    data: Value,
    title: String,
    webpage_url: String,
}

impl ResolvedTrack {
    fn from_data(data: Value, fallback: &str) -> Self {
        let title = text(&data, "title").unwrap_or_else(|| fallback.to_string());
        let webpage_url = text(&data, "webpage_url").unwrap_or_else(|| fallback.to_string());
        Self {
            data,
            title,
            webpage_url,
        }
    }
}

/// Mulai memutar sebuah track, catat riwayat, dan umumkan now playing.
async fn start_player(
    guild_id: GuildId,
    serenity_ctx: &serenity::Context,
    music: &Arc<MusicState>,
    call: &Arc<Mutex<Call>>,
    track_info: ResolvedTrack,
    requester: Option<Requester>,
) -> Result<(), String> {
    let input = build_input(music.http.clone(), &track_info.data)?;
    let volume = music.volume(guild_id).await;
    let track = Track::new(input).volume(volume);
    let handle = call.lock().await.play(track);

    music
        .set_current(
            guild_id,
            Some(CurrentTrack {
                title: track_info.title.clone(),
                webpage_url: track_info.webpage_url.clone(),
                data: track_info.data.clone(),
                requester: requester.clone(),
                handle,
            }),
        )
        .await;
    music.history_push(guild_id, track_info.title.clone()).await;

    let embed = build_current_embed(
        &track_info.data,
        &track_info.title,
        &track_info.webpage_url,
        requester.as_ref(),
    );
    if let Some(channel_id) = music.channel(guild_id).await {
        send_voice_error(serenity_ctx, channel_id, embed).await;
    }
    Ok(())
}

/// Resolve dan putar satu `QueueEntry` (padanan `_play_entry`).
async fn play_entry(
    guild_id: GuildId,
    serenity_ctx: &serenity::Context,
    music: &Arc<MusicState>,
    call: &Arc<Mutex<Call>>,
    entry: QueueEntry,
) {
    let mut handed_off = false;
    music.cancel_disconnect(guild_id).await;

    let channel_id = music.channel(guild_id).await;

    if call.lock().await.current_channel().is_none() {
        return;
    }

    let _typing = channel_id.map(|channel_id| channel_id.start_typing(&serenity_ctx.http));

    let result = async {
        let resolved = resolve_search(&music.http, &entry.url).await?;
        run_ytdlp(&resolved).await
    }
    .await;

    match result {
        Ok(value) if value.get("entries").is_some() => {
            let entries: Vec<Value> = value
                .get("entries")
                .and_then(|entries| entries.as_array())
                .map(|entries| {
                    entries
                        .iter()
                        .filter(|entry| !entry.is_null())
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();

            let requester = entry.requester.clone();

            if entries.is_empty() {
                if let Some(channel_id) = channel_id {
                    send_voice_error(
                        serenity_ctx,
                        channel_id,
                        err("❌ Tidak ada lagu yang bisa diputar.", None),
                    )
                    .await;
                }
                music.set_transitioning(guild_id, false).await;
                play_next_boxed(guild_id, serenity_ctx, music).await;
                handed_off = true;
            } else {
                for candidate in entries.iter().skip(1).rev() {
                    let url = entry_url(candidate);
                    let title = text(candidate, "title").unwrap_or_else(|| url.clone());
                    music
                        .push_front(
                            guild_id,
                            QueueEntry {
                                url,
                                title,
                                requester: requester.clone(),
                            },
                        )
                        .await;
                }

                let first = &entries[0];
                let first_url = entry_url(first);

                match run_ytdlp(&first_url).await {
                    Ok(data) => {
                        if music.is_active(guild_id).await {
                            let first_title =
                                text(first, "title").unwrap_or_else(|| first_url.clone());
                            music
                                .push_front(
                                    guild_id,
                                    QueueEntry {
                                        url: first_url.clone(),
                                        title: first_title,
                                        requester: requester.clone(),
                                    },
                                )
                                .await;
                        } else {
                            let track_info = ResolvedTrack::from_data(data, &first_url);
                            if let Err(error) = start_player(
                                guild_id,
                                serenity_ctx,
                                music,
                                call,
                                track_info,
                                requester.clone(),
                            )
                            .await
                            {
                                tracing::error!(error = %error, "gagal memulai playback");
                                if let Some(channel_id) = channel_id {
                                    send_voice_error(
                                        serenity_ctx,
                                        channel_id,
                                        err(
                                            format!(
                                                "Maaf, lagunya gagal diputar: {error}. Saya lanjut ke lagu berikutnya, {HONORIFIC}."
                                            ),
                                            Some("⚠️ Gangguan Pemutaran"),
                                        ),
                                    )
                                    .await;
                                }
                            }
                        }
                    }
                    Err(error) => {
                        if let Some(channel_id) = channel_id {
                            send_voice_error(
                                serenity_ctx,
                                channel_id,
                                err(
                                    format!(
                                        "Maaf, lagunya gagal diputar: {error}. Saya lanjut ke lagu berikutnya, {HONORIFIC}."
                                    ),
                                    Some("⚠️ Gangguan Pemutaran"),
                                ),
                            )
                            .await;
                        }
                        handed_off = true;
                        music.set_transitioning(guild_id, false).await;
                        play_next_boxed(guild_id, serenity_ctx, music).await;
                    }
                }
            }
        }
        Ok(data) => {
            if music.is_active(guild_id).await {
                music.push_front(guild_id, entry).await;
            } else {
                let track_info = ResolvedTrack::from_data(data, &entry.title);
                if let Err(error) = start_player(
                    guild_id,
                    serenity_ctx,
                    music,
                    call,
                    track_info,
                    entry.requester.clone(),
                )
                .await
                {
                    tracing::error!(error = %error, "gagal memulai playback");
                    if let Some(channel_id) = channel_id {
                        send_voice_error(
                            serenity_ctx,
                            channel_id,
                            err(
                                format!(
                                    "Maaf, lagunya gagal diputar: {error}. Saya lanjut ke lagu berikutnya, {HONORIFIC}."
                                ),
                                Some("⚠️ Gangguan Pemutaran"),
                            ),
                        )
                        .await;
                    }
                }
            }
        }
        Err(error) => {
            if let Some(channel_id) = channel_id {
                send_voice_error(
                    serenity_ctx,
                    channel_id,
                    err(
                        format!(
                            "Maaf, lagunya gagal diputar: {error}. Saya lanjut ke lagu berikutnya, {HONORIFIC}."
                        ),
                        Some("⚠️ Gangguan Pemutaran"),
                    ),
                )
                .await;
            }
            handed_off = true;
            music.set_transitioning(guild_id, false).await;
            play_next_boxed(guild_id, serenity_ctx, music).await;
        }
    }

    if !handed_off {
        music.set_transitioning(guild_id, false).await;
    }
}

// ────────────────────────────────────────────────────────────
// Event handler songbird
// ────────────────────────────────────────────────────────────

struct EndHandler {
    guild_id: GuildId,
    serenity_ctx: serenity::Context,
    music: Arc<MusicState>,
}

#[serenity::async_trait]
impl EventHandler for EndHandler {
    async fn act(&self, _event: &EventContext<'_>) -> Option<Event> {
        let guild_id = self.guild_id;
        let serenity_ctx = self.serenity_ctx.clone();
        let music = self.music.clone();
        tokio::spawn(async move {
            play_next(guild_id, &serenity_ctx, &music).await;
        });
        None
    }
}

struct ErrorHandler {
    guild_id: GuildId,
    serenity_ctx: serenity::Context,
    music: Arc<MusicState>,
}

#[serenity::async_trait]
impl EventHandler for ErrorHandler {
    async fn act(&self, _event: &EventContext<'_>) -> Option<Event> {
        tracing::warn!(guild_id = %self.guild_id, "playback error");
        if let Some(channel_id) = self.music.channel(self.guild_id).await {
            let embed = warn("⚠️ Maaf, ada gangguan saat memutar lagu.", None);
            send_voice_error(&self.serenity_ctx, channel_id, embed).await;
        }
        None
    }
}

/// Daftarkan handler End/Error sekali per voice call.
async fn register_handlers(
    music: &Arc<MusicState>,
    serenity_ctx: &serenity::Context,
    guild_id: GuildId,
    call: &Arc<Mutex<Call>>,
) {
    let mut guard = call.lock().await;
    guard.remove_all_global_events();
    guard.add_global_event(
        Event::Track(TrackEvent::End),
        EndHandler {
            guild_id,
            serenity_ctx: serenity_ctx.clone(),
            music: music.clone(),
        },
    );
    guard.add_global_event(
        Event::Track(TrackEvent::Error),
        ErrorHandler {
            guild_id,
            serenity_ctx: serenity_ctx.clone(),
            music: music.clone(),
        },
    );
}

// ────────────────────────────────────────────────────────────
// Helper voice
// ────────────────────────────────────────────────────────────

fn author_voice_channel(ctx: &Context<'_>, user_id: UserId) -> Option<ChannelId> {
    let guild_id = ctx.guild_id()?;
    ctx.serenity_context()
        .cache
        .guild(guild_id)?
        .voice_states
        .get(&user_id)
        .and_then(|state| state.channel_id)
}

async fn voice_manager(ctx: &Context<'_>) -> Option<Arc<songbird::Songbird>> {
    songbird::get(ctx.serenity_context()).await
}

async fn current_call(ctx: &Context<'_>) -> Option<Arc<Mutex<Call>>> {
    let guild_id = ctx.guild_id()?;
    voice_manager(ctx).await?.get(guild_id)
}

/// True hanya jika author berada di voice channel yang sama dengan bot.
async fn same_voice(ctx: Context<'_>) -> bool {
    if ctx.guild_id().is_none() {
        return false;
    }
    let author_channel = author_voice_channel(&ctx, ctx.author().id);
    let author_voice: Option<songbird::id::ChannelId> = author_channel.map(Into::into);
    let bot_channel = match current_call(&ctx).await {
        Some(call) => call.lock().await.current_channel(),
        None => None,
    };

    if author_channel.is_none() || bot_channel.is_none() || author_voice != bot_channel {
        ctx.send(poise::CreateReply::default().embed(err(
            "❌ Kamu harus ada di voice channel yang sama dengan saya.",
            None,
        )))
        .await
        .ok();
        return false;
    }
    true
}

/// Auto-join voice channel milik author (padanan `_join_author_channel`).
async fn join_author_channel(ctx: Context<'_>) -> Option<Arc<Mutex<Call>>> {
    let guild_id = ctx.guild_id()?;
    let Some(channel_id) = author_voice_channel(&ctx, ctx.author().id) else {
        ctx.send(
            poise::CreateReply::default().embed(err("❌ Masuk ke voice channel dulu ya.", None)),
        )
        .await
        .ok();
        return None;
    };

    let Some(manager) = voice_manager(&ctx).await else {
        ctx.send(poise::CreateReply::default().embed(err(
            "❌ Sistem voice belum siap. Coba lagi sebentar lagi ya.",
            None,
        )))
        .await
        .ok();
        return None;
    };

    match manager.get(guild_id) {
        Some(call) => {
            let mut guard = call.lock().await;
            if guard.current_channel() != Some(channel_id.into()) {
                if let Err(error) = guard.join(channel_id).await {
                    tracing::error!(error = %error, "gagal pindah voice channel");
                    drop(guard);
                    ctx.send(poise::CreateReply::default().embed(err(
                        format!("❌ Gagal bergabung ke voice channel: {error}"),
                        Some("⚠️ Gagal Join Voice"),
                    )))
                    .await
                    .ok();
                    return None;
                }
            }
            drop(guard);
            register_handlers(&ctx.data().music, ctx.serenity_context(), guild_id, &call).await;
            Some(call)
        }
        None => match manager.join(guild_id, channel_id).await {
            Ok(call) => {
                register_handlers(&ctx.data().music, ctx.serenity_context(), guild_id, &call).await;
                Some(call)
            }
            Err(error) => {
                tracing::error!(error = %error, "gagal join voice channel");
                ctx.send(poise::CreateReply::default().embed(err(
                    format!("❌ Maaf {HONORIFIC}, saya gagal bergabung ke voice channel: {error}"),
                    Some("⚠️ Gagal Join Voice"),
                )))
                .await
                .ok();
                None
            }
        },
    }
}

// ────────────────────────────────────────────────────────────
// Commands
// ────────────────────────────────────────────────────────────

/// Bot masuk ke voice channel kamu
#[poise::command(prefix_command, slash_command, rename = "join", guild_only)]
pub async fn join(ctx: Context<'_>) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let Some(channel_id) = author_voice_channel(&ctx, ctx.author().id) else {
        let me = ctx
            .author_member()
            .await
            .map(|member| member.display_name().to_string())
            .unwrap_or_else(|| ctx.author().name.clone());
        ctx.send(
            poise::CreateReply::default()
                .embed(err(format!("❌ **{me}** belum masuk voice channel."), None)),
        )
        .await?;
        return Ok(());
    };

    let Some(manager) = voice_manager(&ctx).await else {
        return Ok(());
    };

    match manager.get(guild_id) {
        Some(call) => {
            if let Err(error) = call.lock().await.join(channel_id).await {
                tracing::error!(error = %error, "gagal join voice");
                ctx.send(poise::CreateReply::default().embed(err(
                    format!("❌ Gagal bergabung ke voice channel: {error}"),
                    Some("⚠️ Gagal Join Voice"),
                )))
                .await?;
                return Ok(());
            }
            let music = ctx.data().music.clone();
            register_handlers(&music, ctx.serenity_context(), guild_id, &call).await;
        }
        None => {
            let call = match manager.join(guild_id, channel_id).await {
                Ok(call) => call,
                Err(error) => {
                    tracing::error!(error = %error, "gagal join voice");
                    ctx.send(poise::CreateReply::default().embed(err(
                        format!(
                            "❌ Maaf {HONORIFIC}, saya gagal bergabung ke voice channel: {error}"
                        ),
                        Some("⚠️ Gagal Join Voice"),
                    )))
                    .await?;
                    return Ok(());
                }
            };
            let music = ctx.data().music.clone();
            register_handlers(&music, ctx.serenity_context(), guild_id, &call).await;
        }
    }

    let name = ctx
        .serenity_context()
        .cache
        .guild(guild_id)
        .and_then(|guild| guild.channels.get(&channel_id).cloned())
        .map(|channel| channel.name.clone())
        .unwrap_or_default();

    ctx.send(poise::CreateReply::default().embed(ok(
        format!("🟢 Siap, {HONORIFIC}. Gabung ke **{name}**."),
        None,
    )))
    .await?;
    Ok(())
}

/// Putar lagu / playlist dari YouTube, Spotify, SoundCloud, atau cari berdasarkan judul
#[poise::command(prefix_command, slash_command, rename = "play", guild_only)]
pub async fn play(
    ctx: Context<'_>,
    #[description = "Judul lagu atau URL"]
    #[rest]
    search: String,
) -> Result<(), Error> {
    let Some(call) = join_author_channel(ctx).await else {
        return Ok(());
    };
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };

    let music = ctx.data().music.clone();
    music.set_channel(guild_id, ctx.channel_id()).await;
    let requester = Some(Requester::from(ctx.author()));

    if search.contains("spotify.com") {
        let embed = CreateEmbed::new()
            .description(format!(
                "🔍 Spotify terdeteksi, {HONORIFIC}. Saya cari padanannya di YouTube..."
            ))
            .color(SPOTIFY_COLOR);
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
    }

    let _typing = ctx.defer_or_broadcast().await;

    let resolved = match resolve_search(&music.http, &search).await {
        Ok(resolved) => resolved,
        Err(error) => {
            ctx.send(poise::CreateReply::default().embed(err(
                format!("❌ Maaf, lagunya gagal diputar: {error}"),
                None,
            )))
            .await?;
            return Ok(());
        }
    };

    let mut result = match run_ytdlp(&resolved).await {
        Ok(result) => result,
        Err(error) => {
            ctx.send(poise::CreateReply::default().embed(err(
                format!("❌ Maaf, lagunya gagal diputar: {error}"),
                None,
            )))
            .await?;
            return Ok(());
        }
    };

    // Hasil dengan satu entri sebenarnya lagu tunggal — ekstrak ulang penuh.
    if let Some(entries) = result.get("entries").and_then(|entries| entries.as_array()) {
        let candidates: Vec<Value> = entries
            .iter()
            .filter(|entry| !entry.is_null())
            .cloned()
            .collect();
        if candidates.len() == 1 {
            let single_url = entry_url(&candidates[0]);
            match run_ytdlp(&single_url).await {
                Ok(data) => result = data,
                Err(error) => {
                    ctx.send(poise::CreateReply::default().embed(err(
                        format!("❌ Maaf, lagunya gagal diputar: {error}"),
                        None,
                    )))
                    .await?;
                    return Ok(());
                }
            }
        }
    }

    if result.get("entries").is_some() {
        handle_playlist(ctx, guild_id, &call, &music, &result, requester.as_ref()).await?;
    } else {
        handle_single(
            ctx,
            guild_id,
            &call,
            &music,
            result,
            &search,
            requester.as_ref(),
        )
        .await?;
    }

    Ok(())
}

async fn handle_playlist(
    ctx: Context<'_>,
    guild_id: GuildId,
    call: &Arc<Mutex<Call>>,
    music: &Arc<MusicState>,
    result: &Value,
    requester: Option<&Requester>,
) -> Result<(), Error> {
    let entries: Vec<Value> = result
        .get("entries")
        .and_then(|entries| entries.as_array())
        .map(|entries| {
            entries
                .iter()
                .filter(|entry| !entry.is_null())
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    let playlist_thumbnail = text(result, "thumbnail");

    if entries.is_empty() {
        ctx.send(poise::CreateReply::default().embed(err(
            format!("❌ Playlist-nya kosong atau tidak ada lagu yang bisa diputar, {HONORIFIC}."),
            None,
        )))
        .await?;
        return Ok(());
    }

    if music.is_active(guild_id).await || music.is_transitioning(guild_id).await {
        for entry in &entries {
            let url = entry_url(entry);
            let title = text(entry, "title").unwrap_or_else(|| url.clone());
            music
                .push_back(
                    guild_id,
                    QueueEntry {
                        url,
                        title,
                        requester: requester.cloned(),
                    },
                )
                .await;
        }

        let first = &entries[0];
        let first_title = text(first, "title").unwrap_or_else(|| "Unknown Track".to_string());
        let first_url = entry_url(first);
        let metadata = extract_song_metadata(first);
        let queue_len = music.queue_len(guild_id).await;

        let mut embed = CreateEmbed::new()
            .title("🎶 Playlist Masuk Antrian")
            .description(format!(
                "**{}** lagu dari playlist sudah masuk antrian, {HONORIFIC}.\n\nPertama: **[{first_title}]({first_url})**",
                entries.len()
            ))
            .color(COLOR_OK)
            .field("Artis", metadata["artist"].clone(), true)
            .field("Album", metadata["album"].clone(), true)
            .field("Durasi", metadata["duration"].clone(), true)
            .field("Total Lagu", entries.len().to_string(), true)
            .field(
                "Posisi Antrian",
                format!("#{} s/d #{}", queue_len + 1 - entries.len(), queue_len),
                true,
            );

        if let Some(thumbnail) = text(first, "thumbnail").or(playlist_thumbnail) {
            embed = embed.thumbnail(thumbnail);
        }
        if let Some(requester) = requester {
            embed = embed.footer(
                CreateEmbedFooter::new(format!(
                    "Atas permintaan {} • Total antrian: {queue_len}",
                    requester.name
                ))
                .icon_url(requester.avatar.clone()),
            );
        }

        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    }

    let first = &entries[0];
    let first_url = entry_url(first);
    for entry in entries.iter().skip(1) {
        let url = entry_url(entry);
        let title = text(entry, "title").unwrap_or_else(|| url.clone());
        music
            .push_back(
                guild_id,
                QueueEntry {
                    url,
                    title,
                    requester: requester.cloned(),
                },
            )
            .await;
    }

    let started = async {
        let data = run_ytdlp(&first_url).await?;
        let input = build_input(music.http.clone(), &data)?;
        let volume = music.volume(guild_id).await;
        let track = Track::new(input).volume(volume);
        let handle = call.lock().await.play(track);

        let title = text(&data, "title").unwrap_or_else(|| first_url.clone());
        let webpage = text(&data, "webpage_url").unwrap_or_else(|| first_url.clone());
        music.history_push(guild_id, title.clone()).await;
        music
            .set_current(
                guild_id,
                Some(CurrentTrack {
                    title: title.clone(),
                    webpage_url: webpage.clone(),
                    data: data.clone(),
                    requester: requester.cloned(),
                    handle,
                }),
            )
            .await;

        let metadata = extract_song_metadata(&data);
        let mut embed = CreateEmbed::new()
            .title("🎶 Sedang Diputar (Playlist)")
            .description(format!("**[{title}]({webpage})**"))
            .color(COLOR_INFO)
            .field("Artis", metadata["artist"].clone(), true)
            .field("Album", metadata["album"].clone(), true)
            .field("Uploader", metadata["uploader"].clone(), true)
            .field("Durasi", metadata["duration"].clone(), true)
            .field(
                "Playlist",
                format!("Sisa **{}** lagu sudah antri.", entries.len() - 1),
                false,
            );

        if let Some(thumbnail) = text(&data, "thumbnail") {
            embed = embed.thumbnail(thumbnail);
        }
        if let Some(requester) = requester {
            embed = embed.footer(
                CreateEmbedFooter::new(format!("Atas permintaan {}", requester.name))
                    .icon_url(requester.avatar.clone()),
            );
        }

        Ok::<CreateEmbed, String>(embed)
    }
    .await;

    match started {
        Ok(embed) => {
            ctx.send(poise::CreateReply::default().embed(embed)).await?;
        }
        Err(error) => {
            ctx.send(poise::CreateReply::default().embed(err(
                format!("❌ Maaf, playlist-nya gagal dijalankan, {HONORIFIC}: {error}"),
                None,
            )))
            .await?;
        }
    }

    Ok(())
}

async fn handle_single(
    ctx: Context<'_>,
    guild_id: GuildId,
    call: &Arc<Mutex<Call>>,
    music: &Arc<MusicState>,
    data: Value,
    search: &str,
    requester: Option<&Requester>,
) -> Result<(), Error> {
    let title = text(&data, "title").unwrap_or_else(|| search.to_string());
    let webpage = text(&data, "webpage_url").unwrap_or_else(|| search.to_string());

    if music.is_active(guild_id).await || music.is_transitioning(guild_id).await {
        music
            .push_back(
                guild_id,
                QueueEntry {
                    url: webpage.clone(),
                    title: title.clone(),
                    requester: requester.cloned(),
                },
            )
            .await;
        let queue_len = music.queue_len(guild_id).await;
        let embed = build_added_embed(&data, &title, &webpage, requester, queue_len);
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
    } else if let Err(error) = start_player(
        guild_id,
        ctx.serenity_context(),
        music,
        call,
        ResolvedTrack {
            data,
            title,
            webpage_url: webpage,
        },
        requester.cloned(),
    )
    .await
    {
        tracing::error!(error = %error, "gagal memulai playback");
        ctx.send(poise::CreateReply::default().embed(err(
            format!("❌ Maaf, lagunya gagal diputar: {error}"),
            Some("⚠️ Gangguan Pemutaran"),
        )))
        .await?;
    }

    Ok(())
}

/// Pause lagu yang sedang diputar
#[poise::command(prefix_command, slash_command, rename = "pause", guild_only)]
pub async fn pause(ctx: Context<'_>) -> Result<(), Error> {
    if !same_voice(ctx).await {
        return Ok(());
    }
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let music = ctx.data().music.clone();

    match music.current(guild_id).await {
        Some(current) if music.is_active(guild_id).await => {
            let _ = current.handle.pause();
            ctx.send(poise::CreateReply::default().embed(warn(
                format!("⏸️ Lagu dijeda — **{}**", current.title),
                None,
            )))
            .await?;
        }
        _ => {
            ctx.send(poise::CreateReply::default().embed(err(
                "❌ Belum ada lagu yang diputar. Coba `!j play` dulu.",
                None,
            )))
            .await?;
        }
    }
    Ok(())
}

/// Lanjutkan lagu yang dijeda
#[poise::command(prefix_command, slash_command, rename = "resume", guild_only)]
pub async fn resume(ctx: Context<'_>) -> Result<(), Error> {
    if !same_voice(ctx).await {
        return Ok(());
    }
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let music = ctx.data().music.clone();

    if music.is_paused(guild_id).await {
        if let Some(current) = music.current(guild_id).await {
            let _ = current.handle.play();
            ctx.send(poise::CreateReply::default().embed(ok(
                format!("▶️ Lagu dilanjutkan — **{}**", current.title),
                None,
            )))
            .await?;
            return Ok(());
        }
        ctx.send(
            poise::CreateReply::default()
                .embed(ok(format!("▶️ Lagu dilanjutkan, {HONORIFIC}."), None)),
        )
        .await?;
        return Ok(());
    }

    ctx.send(
        poise::CreateReply::default().embed(err("❌ Tidak ada lagu yang sedang dijeda.", None)),
    )
    .await?;
    Ok(())
}

/// Lewati lagu yang sedang diputar
#[poise::command(prefix_command, slash_command, rename = "skip", guild_only)]
pub async fn skip(ctx: Context<'_>) -> Result<(), Error> {
    if !same_voice(ctx).await {
        return Ok(());
    }
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let music = ctx.data().music.clone();

    if music.is_active(guild_id).await {
        if let Some(call) = current_call(&ctx).await {
            call.lock().await.stop();
        }
        ctx.send(poise::CreateReply::default().embed(info("⏭️ Lagu dilewati.", None)))
            .await?;
    } else {
        ctx.send(
            poise::CreateReply::default().embed(err("❌ Belum ada lagu yang bisa dilewati.", None)),
        )
        .await?;
    }
    Ok(())
}

/// Stop dan bot keluar dari voice channel
#[poise::command(prefix_command, slash_command, rename = "stop", guild_only)]
pub async fn stop(ctx: Context<'_>) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let Some(call) = current_call(&ctx).await else {
        ctx.send(
            poise::CreateReply::default().embed(err("❌ Saya belum masuk voice channel.", None)),
        )
        .await?;
        return Ok(());
    };

    if !same_voice(ctx).await {
        return Ok(());
    }

    let music = ctx.data().music.clone();
    music.cancel_disconnect(guild_id).await;
    music.clear_queue(guild_id).await;
    music.set_current(guild_id, None).await;
    let _ = call.lock().await.leave().await;

    let embed = CreateEmbed::new()
        .description("⏹️ Oke, saya keluar dan antrian sudah dibersihkan.")
        .color(COLOR_DANGER);
    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Tampilkan antrian lagu saat ini
#[poise::command(prefix_command, slash_command, rename = "queue", guild_only)]
pub async fn queue(ctx: Context<'_>) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let music = ctx.data().music.clone();
    let entries = music.queue_snapshot(guild_id).await;

    let mut description = String::new();
    match music.current(guild_id).await {
        Some(current) if music.is_active(guild_id).await => {
            description.push_str(&format!(
                "**Sedang Diputar:**\n🎶 **{}**\n\n",
                current.title
            ));
        }
        _ => {
            description.push_str("**Sedang Diputar:**\n💤 Belum ada lagu yang diputar.\n\n");
        }
    }

    if entries.is_empty() {
        description.push_str("**Berikutnya:**\n📭 Antrian masih kosong.");
    } else {
        let lines: Vec<String> = entries
            .iter()
            .take(15)
            .enumerate()
            .map(|(index, entry)| {
                let requester = match &entry.requester {
                    Some(requester) => format!(" (atas permintaan {})", requester.mention()),
                    None => String::new(),
                };
                format!("`{}.` {}{requester}", index + 1, entry.title)
            })
            .collect();

        let mut queue_text = lines.join("\n");
        if entries.len() > 15 {
            queue_text += &format!("\n... dan **{}** lagu lainnya.", entries.len() - 15);
        }
        description.push_str(&format!("**Berikutnya:**\n{queue_text}"));
    }

    let embed = CreateEmbed::new()
        .title("📋 Daftar Antrian")
        .description(description)
        .color(COLOR_INFO)
        .footer(
            CreateEmbedFooter::new(format!("Total antrian: {} lagu", entries.len()))
                .icon_url(ctx.author().face()),
        );

    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Tampilkan lagu yang sedang diputar
#[poise::command(
    prefix_command,
    slash_command,
    rename = "nowplaying",
    aliases("np"),
    guild_only
)]
pub async fn nowplaying(ctx: Context<'_>) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let music = ctx.data().music.clone();

    match music.current(guild_id).await {
        Some(current) if music.is_active(guild_id).await => {
            let requester = current
                .requester
                .clone()
                .unwrap_or_else(|| Requester::from(ctx.author()));
            let embed = build_current_embed(
                &current.data,
                &current.title,
                &current.webpage_url,
                Some(&requester),
            );
            ctx.send(poise::CreateReply::default().embed(embed)).await?;
        }
        _ => {
            ctx.send(poise::CreateReply::default().embed(err(
                "❌ Belum ada lagu yang diputar. Coba `!j play` dulu.",
                None,
            )))
            .await?;
        }
    }
    Ok(())
}

/// Tampilkan history lagu yang sudah diputar
#[poise::command(
    prefix_command,
    slash_command,
    rename = "playinghistory",
    aliases("ph"),
    guild_only
)]
pub async fn playing_history(ctx: Context<'_>) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };
    let music = ctx.data().music.clone();
    let history = music.history_snapshot(guild_id).await;

    if history.is_empty() {
        ctx.send(
            poise::CreateReply::default()
                .embed(info("📭 Belum ada lagu yang pernah diputar.", None)),
        )
        .await?;
        return Ok(());
    }

    let mut description = history
        .iter()
        .rev()
        .take(15)
        .enumerate()
        .map(|(index, title)| format!("`{}.` {title}", index + 1))
        .collect::<Vec<String>>()
        .join("\n");

    if history.len() > 15 {
        description += &format!("\n... dan **{}** lagu lainnya.", history.len() - 15);
    }

    let embed = CreateEmbed::new()
        .title("📜 Riwayat Pemutaran")
        .description(description)
        .color(COLOR_INFO)
        .footer(
            CreateEmbedFooter::new(format!("Total riwayat: {} lagu", history.len()))
                .icon_url(ctx.author().face()),
        );

    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Atur volume (0-100)
#[poise::command(prefix_command, slash_command, rename = "volume", guild_only)]
pub async fn volume(
    ctx: Context<'_>,
    #[description = "Volume 0-100"] vol: i64,
) -> Result<(), Error> {
    if !same_voice(ctx).await {
        return Ok(());
    }
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };

    if !(0..=100).contains(&vol) {
        ctx.send(
            poise::CreateReply::default().embed(err("❌ Volume harus di antara 0 dan 100.", None)),
        )
        .await?;
        return Ok(());
    }

    let music = ctx.data().music.clone();
    music.set_volume(guild_id, vol as f32 / 100.0).await;

    if let Some(current) = music.current(guild_id).await {
        let _ = current.handle.set_volume(vol as f32 / 100.0);
    }

    ctx.send(poise::CreateReply::default().embed(info(
        format!("🔊 Volume diatur ke **{vol}%**, {HONORIFIC}."),
        None,
    )))
    .await?;
    Ok(())
}

/// Hapus semua lagu dalam antrian
#[poise::command(
    prefix_command,
    slash_command,
    rename = "clearqueue",
    aliases("cq"),
    guild_only
)]
pub async fn clearqueue(ctx: Context<'_>) -> Result<(), Error> {
    if !same_voice(ctx).await {
        return Ok(());
    }
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };

    let music = ctx.data().music.clone();
    music.clear_queue(guild_id).await;

    let embed = CreateEmbed::new()
        .description(format!("🗑️ Antrian sudah dikosongkan, {HONORIFIC}."))
        .color(COLOR_DANGER);
    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Hapus lagu tertentu dari antrian berdasarkan nomor
#[poise::command(prefix_command, slash_command, rename = "remove", guild_only)]
pub async fn remove(
    ctx: Context<'_>,
    #[description = "Nomor lagu di antrian"] index: i64,
) -> Result<(), Error> {
    if !same_voice(ctx).await {
        return Ok(());
    }
    let Some(guild_id) = ctx.guild_id() else {
        return Ok(());
    };

    let music = ctx.data().music.clone();
    let queue_len = music.queue_len(guild_id).await;

    if index >= 1 && (index as usize) <= queue_len {
        if let Some(removed) = music.remove_at(guild_id, index as usize - 1).await {
            let embed = CreateEmbed::new()
                .description(format!(
                    "🗑️ Dihapus dari antrian, {HONORIFIC}: {}",
                    removed.title
                ))
                .color(COLOR_DANGER);
            ctx.send(poise::CreateReply::default().embed(embed)).await?;
            return Ok(());
        }
    }

    ctx.send(
        poise::CreateReply::default().embed(err("❌ Nomor lagu itu tidak ada di antrian.", None)),
    )
    .await?;
    Ok(())
}
