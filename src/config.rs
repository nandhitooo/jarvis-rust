//! Static configuration values shared across the bot.

/// Sapaan yang dipakai bot untuk memanggil penggunanya.
pub const HONORIFIC: &str = "Boss";

/// Zona waktu untuk menampilkan waktu lokal.
pub const TIMEZONE: &str = "Asia/Jakarta";

/// Cog yang dimuat saat startup, mengikuti isi folder `cogs/` pada versi Python.
pub const COGS: [&str; 5] = ["AI", "Assistant", "Moderation", "Music", "Utility"];

/// Versi library Discord yang dipakai, ditampilkan oleh perintah `stats`.
pub const DISCORD_LIBRARY_VERSION: &str = "0.12.5";

/// Mengembalikan zona waktu yang dikonfigurasi, meniru `ZoneInfo(TIMEZONE)`.
pub fn timezone() -> Option<chrono_tz::Tz> {
    TIMEZONE.parse().ok()
}
