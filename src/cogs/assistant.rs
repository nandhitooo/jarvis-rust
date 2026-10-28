//! Assistant cog: reminders, timers, weather, calculator, and system info.

use std::sync::{Arc, Mutex};
use std::time::Duration as StdDuration;

use chrono::{DateTime, Duration, Utc};
use poise::serenity_prelude as serenity;
use serenity::all::{CreateEmbed, CreateEmbedFooter, CreateMessage, Mentionable, UserId};
use sysinfo::{Disks, System};

use crate::config::HONORIFIC;
use crate::helpers::{info, COLOR_INFO};
use crate::{Context, Data, Error};

/// Satu pengingat yang belum tiba waktunya (`self.reminders` pada Python).
#[derive(Clone)]
pub struct Reminder {
    pub user_id: UserId,
    pub task: String,
    pub time: DateTime<Utc>,
}

/// Semua perintah pada cog asisten.
pub fn commands() -> Vec<poise::Command<Data, Error>> {
    vec![remind(), timer(), weather(), calc(), sysinfo()]
}

/// Daftar pengingat global, dipakai perintah `remind` dan loop latar belakang.
pub type Reminders = Arc<Mutex<Vec<Reminder>>>;

/// Loop latar belakang 30 detik yang mengirim pengingat (padanan `check_reminders`).
pub fn spawn_reminder_loop(http: Arc<serenity::Http>, reminders: Reminders) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(StdDuration::from_secs(30)).await;

            let now = Utc::now();
            let mut due: Vec<Reminder> = Vec::new();
            {
                let mut guard = reminders.lock().unwrap_or_else(|error| error.into_inner());
                guard.retain(|reminder| {
                    if reminder.time <= now {
                        due.push(reminder.clone());
                        false
                    } else {
                        true
                    }
                });
            }

            for reminder in due {
                let Ok(user) = http.get_user(reminder.user_id).await else {
                    continue;
                };
                let Ok(channel) = user.create_dm_channel(&http).await else {
                    continue;
                };
                let embed = info(
                    format!("Waktunya: **{}**, {HONORIFIC}.", reminder.task),
                    Some("⏰ Pengingat!"),
                );
                let _ = channel
                    .send_message(&http, CreateMessage::new().embed(embed))
                    .await;
            }
        }
    });
}

/// Parse durasi seperti `10m` menjadi [`Duration`], atau `None` jika tidak valid.
fn parse_duration(time_str: &str, allowed_units: &[char]) -> Option<Duration> {
    let lowered = time_str.to_lowercase();
    let mut chars = lowered.chars();
    let unit = chars.next_back()?;
    if !allowed_units.contains(&unit) {
        return None;
    }
    let digits = chars.as_str();
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let amount: i64 = digits.parse().ok()?;
    if amount < 1 {
        return None;
    }
    let seconds = match unit {
        's' => amount,
        'm' => amount.checked_mul(60)?,
        'h' => amount.checked_mul(3600)?,
        'd' => amount.checked_mul(86_400)?,
        _ => return None,
    };
    Some(Duration::seconds(seconds))
}

/// Setel pengingat (contoh: !j remind 10m beli kopi)
#[poise::command(prefix_command, slash_command, rename = "remind")]
pub async fn remind(
    ctx: Context<'_>,
    #[description = "Waktu pengingat, contoh: 10m (s/m/h/d)"] time_str: String,
    #[description = "Tugas yang ingin diingatkan"]
    #[rest]
    task: String,
) -> Result<(), Error> {
    const BAD_FORMAT: &str = "Format waktunya belum tepat. Pakai `s` (detik), `m` (menit), `h` (jam), atau `d` (hari). Contoh: `10m`.";

    let delta = parse_duration(&time_str, &['s', 'm', 'h', 'd']);
    let Some(delta) = delta.filter(|delta| *delta <= Duration::days(30)) else {
        ctx.say(BAD_FORMAT).await?;
        return Ok(());
    };

    let remind_time = Utc::now() + delta;
    {
        let mut guard = ctx
            .data()
            .reminders
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        guard.push(Reminder {
            user_id: ctx.author().id,
            task: task.clone(),
            time: remind_time,
        });
    }

    let embed = info(
        format!("✅ Oke, {HONORIFIC}. Saya ingatkan **{task}** dalam **{time_str}**."),
        None,
    );
    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Setel timer (contoh: !j timer 5m)
#[poise::command(prefix_command, slash_command, rename = "timer")]
pub async fn timer(
    ctx: Context<'_>,
    #[description = "Durasi timer, contoh: 5m (s/m/h)"] time_str: String,
) -> Result<(), Error> {
    const BAD_FORMAT: &str = "Format waktunya belum tepat. Pakai `s`, `m`, atau `h`.";

    let delta = parse_duration(&time_str, &['s', 'm', 'h']);
    let Some(delta) = delta.filter(|delta| *delta <= Duration::hours(6)) else {
        ctx.say(BAD_FORMAT).await?;
        return Ok(());
    };

    let seconds = delta.num_seconds().max(0) as u64;
    ctx.say(format!("⏲️ Sip, {HONORIFIC}. Timer **{time_str}** jalan."))
        .await?;
    tokio::time::sleep(StdDuration::from_secs(seconds)).await;
    ctx.say(format!(
        "🔔 {}, waktunya habis! (**{time_str}**)",
        ctx.author().id.mention()
    ))
    .await?;
    Ok(())
}

/// Persen-encode komponen URL seperti `urllib.parse.quote`.
fn url_encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.as_bytes() {
        let c = *byte as char;
        if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~') {
            out.push(c);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Meniru `str.title()` bawaan Python untuk nama kota.
fn title_case(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut at_word_start = true;
    for c in value.chars() {
        if c.is_alphabetic() {
            if at_word_start {
                out.extend(c.to_uppercase());
            } else {
                out.extend(c.to_lowercase());
            }
            at_word_start = false;
        } else {
            out.push(c);
            at_word_start = true;
        }
    }
    out
}

/// Meniru `round(x, 2)` versi Python saat diconvert ke string (mis. `7.5`, `7.12`).
fn py_round(value: f64, digits: usize) -> String {
    let rounded = format!("{value:.*}", digits);
    let trimmed = rounded.trim_end_matches('0');
    if trimmed.ends_with('.') {
        format!("{trimmed}0")
    } else {
        trimmed.to_string()
    }
}

/// Cek cuaca di suatu kota
#[poise::command(prefix_command, slash_command, rename = "weather")]
pub async fn weather(
    ctx: Context<'_>,
    #[description = "Nama kota"]
    #[rest]
    city: String,
) -> Result<(), Error> {
    let url = format!("https://wttr.in/{}?format=j1", url_encode(&city));

    let _typing = ctx.defer_or_broadcast().await;

    let response = ctx
        .data()
        .http
        .get(&url)
        .timeout(StdDuration::from_secs(10))
        .send()
        .await;

    let Ok(response) = response else {
        ctx.say("Data cuaca tidak bisa saya ambil. Pastikan nama kotanya benar, lalu coba lagi.")
            .await?;
        return Ok(());
    };

    if response.status() != reqwest::StatusCode::OK {
        ctx.say("Data cuaca tidak bisa saya ambil. Pastikan nama kotanya benar.")
            .await?;
        return Ok(());
    }

    let Ok(data) = response.json::<serde_json::Value>().await else {
        ctx.say("Data cuaca tidak bisa saya ambil. Pastikan nama kotanya benar, lalu coba lagi.")
            .await?;
        return Ok(());
    };

    let current = data.get("current_condition").and_then(|c| c.get(0));
    let parsed = current.and_then(|current| {
        Some((
            current.get("temp_C")?.as_str()?.to_string(),
            current
                .get("weatherDesc")?
                .get(0)?
                .get("value")?
                .as_str()?
                .to_string(),
            current.get("humidity")?.as_str()?.to_string(),
            current.get("windspeedKmph")?.as_str()?.to_string(),
        ))
    });

    let Some((temp, desc, humidity, wind)) = parsed else {
        ctx.say("Data cuaca tidak bisa saya ambil. Pastikan nama kotanya benar, lalu coba lagi.")
            .await?;
        return Ok(());
    };

    let embed = CreateEmbed::new()
        .title(format!("🌡️ Laporan Cuaca — {}", title_case(&city)))
        .color(COLOR_INFO)
        .field("Suhu", format!("{temp}°C"), true)
        .field("Kondisi", desc, true)
        .field("Kelembapan", format!("{humidity}%"), true)
        .field("Angin", format!("{wind} km/h"), true)
        .footer(CreateEmbedFooter::new("Data provided by wttr.in"));

    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

// ────────────────────────────────────────────────────────────
// Kalkulator aman (padanan `_safe_eval` Python)
// ────────────────────────────────────────────────────────────

/// Hasil evaluasi: bilangan bulat atau pecahan, sama seperti `int`/`float` Python.
#[derive(Clone, Copy, Debug)]
enum Num {
    Int(i64),
    Float(f64),
}

impl Num {
    fn as_f64(self) -> f64 {
        match self {
            Num::Int(value) => value as f64,
            Num::Float(value) => value,
        }
    }

    fn py_display(self) -> String {
        match self {
            Num::Int(value) => value.to_string(),
            Num::Float(value) => format_py_float(value),
        }
    }
}

/// Format float seperti `str()` Python (menambahkan `.0` bila perlu).
fn format_py_float(value: f64) -> String {
    if value.is_nan() {
        return "nan".to_string();
    }
    if value.is_infinite() {
        return if value > 0.0 { "inf" } else { "-inf" }.to_string();
    }
    let text = format!("{value}");
    if text.contains(['.', 'e', 'E']) {
        text
    } else {
        format!("{text}.0")
    }
}

#[derive(Clone, Copy, Debug)]
enum EvalError {
    ZeroDivision,
    Invalid,
}

fn py_floordiv_int(lhs: i64, rhs: i64) -> Result<i64, EvalError> {
    if rhs == 0 {
        return Err(EvalError::ZeroDivision);
    }
    let quotient = lhs / rhs;
    let remainder = lhs % rhs;
    if remainder != 0 && (remainder < 0) != (rhs < 0) {
        quotient.checked_sub(1).ok_or(EvalError::Invalid)
    } else {
        Ok(quotient)
    }
}

fn py_modulo_int(lhs: i64, rhs: i64) -> Result<i64, EvalError> {
    if rhs == 0 {
        return Err(EvalError::ZeroDivision);
    }
    let remainder = lhs % rhs;
    if remainder != 0 && (remainder < 0) != (rhs < 0) {
        remainder.checked_add(rhs).ok_or(EvalError::Invalid)
    } else {
        Ok(remainder)
    }
}

fn eval_add(lhs: Num, rhs: Num) -> Result<Num, EvalError> {
    match (lhs, rhs) {
        (Num::Int(a), Num::Int(b)) => a.checked_add(b).map(Num::Int).ok_or(EvalError::Invalid),
        _ => Ok(Num::Float(lhs.as_f64() + rhs.as_f64())),
    }
}

fn eval_sub(lhs: Num, rhs: Num) -> Result<Num, EvalError> {
    match (lhs, rhs) {
        (Num::Int(a), Num::Int(b)) => a.checked_sub(b).map(Num::Int).ok_or(EvalError::Invalid),
        _ => Ok(Num::Float(lhs.as_f64() - rhs.as_f64())),
    }
}

fn eval_mul(lhs: Num, rhs: Num) -> Result<Num, EvalError> {
    match (lhs, rhs) {
        (Num::Int(a), Num::Int(b)) => a.checked_mul(b).map(Num::Int).ok_or(EvalError::Invalid),
        _ => Ok(Num::Float(lhs.as_f64() * rhs.as_f64())),
    }
}

fn eval_div(lhs: Num, rhs: Num) -> Result<Num, EvalError> {
    if rhs.as_f64() == 0.0 {
        return Err(EvalError::ZeroDivision);
    }
    Ok(Num::Float(lhs.as_f64() / rhs.as_f64()))
}

fn eval_floordiv(lhs: Num, rhs: Num) -> Result<Num, EvalError> {
    match (lhs, rhs) {
        (Num::Int(a), Num::Int(b)) => py_floordiv_int(a, b).map(Num::Int),
        _ => {
            if rhs.as_f64() == 0.0 {
                return Err(EvalError::ZeroDivision);
            }
            Ok(Num::Float((lhs.as_f64() / rhs.as_f64()).floor()))
        }
    }
}

fn eval_modulo(lhs: Num, rhs: Num) -> Result<Num, EvalError> {
    match (lhs, rhs) {
        (Num::Int(a), Num::Int(b)) => py_modulo_int(a, b).map(Num::Int),
        _ => {
            let (a, b) = (lhs.as_f64(), rhs.as_f64());
            if b == 0.0 {
                return Err(EvalError::ZeroDivision);
            }
            Ok(Num::Float(a - b * (a / b).floor()))
        }
    }
}

fn eval_pow(base: Num, exponent: Num) -> Result<Num, EvalError> {
    if exponent.as_f64().abs() > 100.0 {
        return Err(EvalError::Invalid);
    }
    match (base, exponent) {
        (Num::Int(b), Num::Int(e)) if e >= 0 => {
            let e = u32::try_from(e).map_err(|_| EvalError::Invalid)?;
            b.checked_pow(e).map(Num::Int).ok_or(EvalError::Invalid)
        }
        _ => Ok(Num::Float(base.as_f64().powf(exponent.as_f64()))),
    }
}

struct CalcParser {
    chars: Vec<char>,
    pos: usize,
}

impl CalcParser {
    fn new(input: &str) -> Self {
        Self {
            chars: input.chars().collect(),
            pos: 0,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_next(&self) -> Option<char> {
        self.chars.get(self.pos + 1).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let value = self.peek();
        if value.is_some() {
            self.pos += 1;
        }
        value
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(c) if c.is_whitespace()) {
            self.pos += 1;
        }
    }

    fn parse(mut self) -> Result<Num, EvalError> {
        let value = self.expression()?;
        self.skip_whitespace();
        if self.pos == self.chars.len() {
            Ok(value)
        } else {
            Err(EvalError::Invalid)
        }
    }

    fn expression(&mut self) -> Result<Num, EvalError> {
        let mut left = self.term()?;
        loop {
            self.skip_whitespace();
            match self.peek() {
                Some('+') => {
                    self.bump();
                    left = eval_add(left, self.term()?)?;
                }
                Some('-') => {
                    self.bump();
                    left = eval_sub(left, self.term()?)?;
                }
                _ => return Ok(left),
            }
        }
    }

    fn term(&mut self) -> Result<Num, EvalError> {
        let mut left = self.unary()?;
        loop {
            self.skip_whitespace();
            match self.peek() {
                Some('*') if self.peek_next() == Some('*') => return Ok(left),
                Some('*') => {
                    self.bump();
                    left = eval_mul(left, self.unary()?)?;
                }
                Some('/') if self.peek_next() == Some('/') => {
                    self.bump();
                    self.bump();
                    left = eval_floordiv(left, self.unary()?)?;
                }
                Some('/') => {
                    self.bump();
                    left = eval_div(left, self.unary()?)?;
                }
                Some('%') => {
                    self.bump();
                    left = eval_modulo(left, self.unary()?)?;
                }
                _ => return Ok(left),
            }
        }
    }

    fn unary(&mut self) -> Result<Num, EvalError> {
        self.skip_whitespace();
        match self.peek() {
            Some('+') => {
                self.bump();
                self.unary()
            }
            Some('-') => {
                self.bump();
                match self.unary()? {
                    Num::Int(value) => value.checked_neg().map(Num::Int).ok_or(EvalError::Invalid),
                    Num::Float(value) => Ok(Num::Float(-value)),
                }
            }
            _ => self.power(),
        }
    }

    fn power(&mut self) -> Result<Num, EvalError> {
        let base = self.atom()?;
        self.skip_whitespace();
        if self.peek() == Some('*') && self.peek_next() == Some('*') {
            self.bump();
            self.bump();
            let exponent = self.unary()?;
            eval_pow(base, exponent)
        } else {
            Ok(base)
        }
    }

    fn atom(&mut self) -> Result<Num, EvalError> {
        self.skip_whitespace();
        match self.peek() {
            Some('(') => {
                self.bump();
                let value = self.expression()?;
                self.skip_whitespace();
                if self.bump() != Some(')') {
                    return Err(EvalError::Invalid);
                }
                Ok(value)
            }
            Some(c) if c.is_ascii_digit() || c == '.' => self.number(),
            _ => Err(EvalError::Invalid),
        }
    }

    fn number(&mut self) -> Result<Num, EvalError> {
        let start = self.pos;
        let mut has_dot = false;
        let mut has_exponent = false;

        while let Some(c) = self.peek() {
            if c.is_ascii_digit() || c == '_' {
                self.bump();
            } else if c == '.' && !has_dot && !has_exponent {
                has_dot = true;
                self.bump();
            } else if (c == 'e' || c == 'E') && !has_exponent {
                let saved = self.pos;
                self.bump();
                if matches!(self.peek(), Some('+') | Some('-')) {
                    self.bump();
                }
                if matches!(self.peek(), Some(next) if next.is_ascii_digit()) {
                    has_exponent = true;
                } else {
                    self.pos = saved;
                    break;
                }
            } else {
                break;
            }
        }

        let raw: String = self.chars[start..self.pos].iter().collect();
        if raw.is_empty()
            || raw == "."
            || raw.starts_with('_')
            || raw.ends_with('_')
            || raw.contains("__")
            || raw.contains("_.")
            || raw.contains("._")
        {
            return Err(EvalError::Invalid);
        }
        let cleaned = raw.replace('_', "");
        if has_dot || has_exponent {
            cleaned
                .parse::<f64>()
                .map(Num::Float)
                .map_err(|_| EvalError::Invalid)
        } else {
            cleaned
                .parse::<i64>()
                .map(Num::Int)
                .map_err(|_| EvalError::Invalid)
        }
    }
}

/// Kalkulator matematika
#[poise::command(prefix_command, slash_command, rename = "calc")]
pub async fn calc(
    ctx: Context<'_>,
    #[description = "Ekspresi matematika, contoh: (2+3)*4"]
    #[rest]
    expr: String,
) -> Result<(), Error> {
    let expr_clean = expr.replace('^', "**").trim().to_string();
    if expr_clean.chars().count() > 100 {
        ctx.say("Ekspresinya kepanjangan — maksimal 100 karakter.")
            .await?;
        return Ok(());
    }

    let result = match CalcParser::new(&expr_clean).parse() {
        Ok(value) => value,
        Err(EvalError::ZeroDivision) => {
            ctx.say("Tidak bisa dibagi nol.").await?;
            return Ok(());
        }
        Err(EvalError::Invalid) => {
            ctx.say("Ekspresinya tidak valid. Pakai angka dan operator dasar saja.")
                .await?;
            return Ok(());
        }
    };

    let embed = info(
        format!("**Input:** `{expr}`\n**Hasil:** `{}`", result.py_display()),
        Some("🔢 Kalkulator"),
    );
    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

/// Lihat statistik sistem host bot
#[poise::command(prefix_command, slash_command, rename = "sysinfo", owners_only)]
pub async fn sysinfo(ctx: Context<'_>) -> Result<(), Error> {
    let stats = tokio::task::spawn_blocking(|| {
        let mut system = System::new_all();
        system.refresh_cpu_usage();
        std::thread::sleep(StdDuration::from_millis(500));
        system.refresh_cpu_usage();

        let cpu_usage = system.global_cpu_usage();
        let used_memory = system.used_memory() as f64 / (1024.0 * 1024.0 * 1024.0);
        let total_memory = system.total_memory() as f64 / (1024.0 * 1024.0 * 1024.0);

        let disks = Disks::new_with_refreshed_list();
        let disk_percent = disks
            .list()
            .iter()
            .find(|disk| disk.mount_point() == std::path::Path::new("/"))
            .map(|disk| {
                let total = disk.total_space();
                if total == 0 {
                    0.0
                } else {
                    (total - disk.available_space()) as f64 / total as f64 * 100.0
                }
            })
            .unwrap_or(0.0);

        let os = format!(
            "{} {}",
            System::name().unwrap_or_else(|| "Unknown".to_string()),
            System::kernel_version().unwrap_or_default()
        );
        let ram_percent = if total_memory == 0.0 {
            0.0
        } else {
            used_memory / total_memory * 100.0
        };

        (
            os,
            cpu_usage,
            ram_percent,
            used_memory,
            total_memory,
            disk_percent,
        )
    })
    .await?;

    let (os, cpu_usage, ram_percent, used_memory, total_memory, disk_percent) = stats;

    let embed = CreateEmbed::new()
        .title("🖥️ Status Sistem")
        .color(COLOR_INFO)
        .field("OS", os, false)
        .field("CPU Usage", format!("{cpu_usage:.1}%"), true)
        .field(
            "RAM Usage",
            format!(
                "{ram_percent:.1}% ({} GB / {} GB)",
                py_round(used_memory, 2),
                py_round(total_memory, 2)
            ),
            false,
        )
        .field("Disk Usage", format!("{disk_percent:.1}%"), true)
        .field("Rust Version", env!("CARGO_PKG_RUST_VERSION"), true);

    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}
