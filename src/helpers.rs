//! Shared embed builders and color constants for consistent bot responses.

use poise::serenity_prelude::CreateEmbed;

pub const COLOR_ERROR: u32 = 0xFF_33_33;
pub const COLOR_OK: u32 = 0x2E_CC_71;
pub const COLOR_WARN: u32 = 0xF1_C4_0F;
pub const COLOR_INFO: u32 = 0x00_E5_FF;
pub const COLOR_DANGER: u32 = 0xE7_4C_3C;

fn make(color: u32, text: impl Into<String>, title: Option<&str>) -> CreateEmbed {
    let embed = CreateEmbed::new().description(text).color(color);
    match title {
        Some(title) => embed.title(title),
        None => embed,
    }
}

/// Embed merah untuk pesan error.
pub fn err(text: impl Into<String>, title: Option<&str>) -> CreateEmbed {
    make(COLOR_ERROR, text, title)
}

/// Embed hijau untuk pesan sukses.
pub fn ok(text: impl Into<String>, title: Option<&str>) -> CreateEmbed {
    make(COLOR_OK, text, title)
}

/// Embed kuning untuk pesan peringatan.
pub fn warn(text: impl Into<String>, title: Option<&str>) -> CreateEmbed {
    make(COLOR_WARN, text, title)
}

/// Embed cyan untuk pesan informasi.
pub fn info(text: impl Into<String>, title: Option<&str>) -> CreateEmbed {
    make(COLOR_INFO, text, title)
}
