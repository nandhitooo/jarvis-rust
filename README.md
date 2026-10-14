# Jarvis (Rust port)

Port Rust dari bot Discord **Jarvis** (versi Python). Semua perintah, alias, embed,
dan teks balasan berbahasa Indonesia dipertahankan sama seperti versi Python.

- Layout modul mengikuti versi Python: `src/{main,config,helpers}.rs` dan
  `src/cogs/{ai,assistant,moderation,music,utility}.rs`.
- Prefix: `!jarvis `, `!j `, `!jarvis`, `!j` (perintah tidak peka huruf besar/kecil,
  tanpa perintah `help` bawaan).
- Mengetik `!jarvis` atau `!j` saja akan menampilkan menu bantuan dengan dropdown
  kategori (timeout 60 detik, komponen dinonaktifkan saat timeout).

## Kebutuhan

| Kebutuhan | Keterangan |
| --- | --- |
| Rust | Edition 2021, `rust-version = 1.99` |
| `cmake` | Dipakai `audiopus_sys` (dependency `songbird`) untuk mengompilasi libopus. **Wajib saat build.** |
| `yt-dlp` | Harus ada di `PATH`. Dipakai untuk resolve metadata dan stream audio. |
| `ffmpeg` | Harus ada di `PATH`. Dipakai untuk dekode/encode audio. |
| Opus | Sudah disediakan `songbird`, tidak perlu memuat `libopus` manual seperti versi Python. |

## Build & jalankan

```bash
cd jarvis-rs
cp .env.example .env      # lalu isi nilainya
cargo build --release
cargo run --release
```

Pemeriksaan statis:

```bash
cargo check
cargo clippy -- -D warnings
```

## Variabel environment

Semua dibaca dari `.env` (lewat `dotenvy`). Nama variabel saja — nilainya tidak
pernah dicetak ke log.

| Variabel | Wajib | Fungsi |
| --- | --- | --- |
| `DISCORD_TOKEN` | Ya | Token bot Discord. Bila kosong, bot berhenti dengan pesan `[Error] DISCORD_TOKEN tidak ditemukan di file .env`. |
| `JARVIS_API_URL` | Tidak | URL custom Jarvis API (endpoint OpenAI-compatible). Bila diisi, backend ini yang dipakai. |
| `JARVIS_API_SECRET` | Tidak | Secret untuk custom Jarvis API. Wajib ada bila `JARVIS_API_URL` diisi, kalau tidak custom API dinonaktifkan. |
| `GROQ_API_KEY` | Tidak | Dipakai bila `JARVIS_API_URL` tidak diatur (langsung ke `https://api.groq.com/openai/v1`). |
| `GROQ_MODEL_1` .. `GROQ_MODEL_3` | Tidak | ID model yang ditawarkan. Harus berpasangan dengan label di bawah. |
| `GROQ_MODEL_LABEL_1` .. `GROQ_MODEL_LABEL_3` | Tidak | Label tampilan model. Bila pasangan model/label kosong semua, dipakai tiga default: `openai/gpt-oss-120b`, `openai/gpt-oss-20b`, `qwen/qwen3.6-27b`. |
| `CUSTOM_IMAGE_API_URL` | Tidak | Endpoint untuk perintah `image`. |
| `CUSTOM_IMAGE_API_KEY` | Tidak | Kunci untuk endpoint gambar di atas. |

## Catatan perilaku

- **State musik dan warning bersifat in-memory** (sama seperti versi Python):
  queue, history (maksimal 100 entri per guild), volume per guild, dan daftar
  warning per (guild, user) hilang saat bot restart.
- **Auto-disconnect**: 5 menit tanpa lagu aktif dan antrian kosong, bot keluar
  dari voice channel.
- **Pemutaran lagu** memakai `yt-dlp` (proses terpisah) untuk mengambil URL stream,
  lalu audio didekode oleh `songbird` + `symphonia`. Tautan Spotify diselesaikan
  lewat scraping Open Graph menjadi pencarian YouTube (`ytsearch:`).
- **`summarize`** menolak alamat privat/loopback (guard SSRF), memeriksa
  `Content-Type` dan `Content-Length`, dan memotong teks pada 12.000 karakter.
- **`calc`** memakai evaluator recursive-descent sendiri (tanpa `eval`), dengan
  grammar yang sama seperti whitelist `ast` versi Python.
- Perintah yang tidak dikenal diabaikan tanpa balasan.

## Struktur

```
src/
├── main.rs                 # entrypoint, load cog, menu bantuan, global error handler
├── config.rs               # HONORIFIC, TIMEZONE, daftar cog
├── helpers.rs              # err/ok/warn/info + konstanta warna
└── cogs/
    ├── ai.rs               # ask, summarize, search, model, image
    ├── assistant.rs        # remind, timer, weather, calc, sysinfo (+ loop pengingat 30 detik)
    ├── moderation.rs       # ban, unban, kick, timeout, untimeout, purge, warn, warnings,
    │                       # clearwarns, slowmode, lock, unlock, mute, unmute
    ├── music.rs            # join, play, pause, resume, skip, stop, queue, nowplaying,
    │                       # playinghistory, volume, clearqueue, remove
    └── utility.rs          # ping, cogs, info, help, invite, uptime, stats, userinfo, serverinfo
```

## Yang tidak bisa diverifikasi tanpa kredensial

Perilaku Discord langsung (gateway, slash/interaction, moderasi nyata) dan
pemutaran suara tidak dapat diuji tanpa token bot, voice channel, serta
`yt-dlp`/`ffmpeg` di `PATH`. Verifikasi otomatis yang dilakukan hanya
`cargo check` dan `cargo clippy -- -D warnings`.
