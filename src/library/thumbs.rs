//! Генерация и кеш thumbnail'ов через ffmpeg (phase 19.b.3).
//! Кеш: `~/.cache/ralume/thumbs/<sha1(path)>.jpg`.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use directories::ProjectDirs;

/// Версия кеша — bump при изменении параметров ffmpeg (scale, формат и т.п.),
/// чтобы старые thumbnails автоматически регенерировались.
const THUMB_CACHE_VERSION: u8 = 2;

/// Путь к thumbnail'у (независимо от его существования).
pub fn thumb_path(video: &Path) -> PathBuf {
    let hash = sha1_hex(video.to_string_lossy().as_bytes());
    cache_dir().join(format!("v{THUMB_CACHE_VERSION}-{hash}.jpg"))
}

/// Убедиться, что thumbnail существует и свежий. Возвращает путь при успехе,
/// `None` если ffmpeg недоступен или упал.
pub fn ensure_thumb(video: &Path) -> Option<PathBuf> {
    let cache = cache_dir();
    std::fs::create_dir_all(&cache).ok()?;
    let thumb = thumb_path(video);

    // Валиден, если thumb существует и новее видео.
    if thumb.is_file() {
        let (Ok(t_meta), Ok(v_meta)) = (std::fs::metadata(&thumb), std::fs::metadata(video)) else {
            return Some(thumb);
        };
        let (Ok(t_mod), Ok(v_mod)) = (t_meta.modified(), v_meta.modified()) else {
            return Some(thumb);
        };
        if t_mod >= v_mod {
            return Some(thumb);
        }
    }

    // ffmpeg -ss 5 -i <video> -frames:v 1 -vf scale=320:-1 <thumb>
    let status = Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error", "-ss", "5"])
        .arg("-i")
        .arg(video)
        .args(["-frames:v", "1", "-vf", "scale=720:-1"])
        .arg(&thumb)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .ok()?;
    if !status.success() {
        // Попробуем ещё раз без -ss на случай, если видео короче 5 с.
        let status2 = Command::new("ffmpeg")
            .args(["-y", "-loglevel", "error"])
            .arg("-i")
            .arg(video)
            .args(["-frames:v", "1", "-vf", "scale=720:-1"])
            .arg(&thumb)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .ok()?;
        if !status2.success() {
            return None;
        }
    }
    Some(thumb)
}

fn cache_dir() -> PathBuf {
    ProjectDirs::from("dev", "local", "Ralume")
        .map(|d| d.cache_dir().join("thumbs"))
        .unwrap_or_else(|| std::env::temp_dir().join("ralume-thumbs"))
}

/// Минимальный sha1 в hex — чтобы не тянуть `sha1`-крейт.
/// Реализация по RFC 3174 (сжато, без bit-flag оптимизаций).
fn sha1_hex(bytes: &[u8]) -> String {
    let mut h0: u32 = 0x67452301;
    let mut h1: u32 = 0xEFCDAB89;
    let mut h2: u32 = 0x98BADCFE;
    let mut h3: u32 = 0x10325476;
    let mut h4: u32 = 0xC3D2E1F0;

    let bitlen = (bytes.len() as u64) * 8;
    let mut msg = bytes.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bitlen.to_be_bytes());

    for chunk in msg.chunks(64) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes(chunk[i * 4..i * 4 + 4].try_into().unwrap());
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }

        let (mut a, mut b, mut c, mut d, mut e) = (h0, h1, h2, h3, h4);

        for i in 0..80 {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A827999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(w[i]);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }

        h0 = h0.wrapping_add(a);
        h1 = h1.wrapping_add(b);
        h2 = h2.wrapping_add(c);
        h3 = h3.wrapping_add(d);
        h4 = h4.wrapping_add(e);
    }

    format!("{h0:08x}{h1:08x}{h2:08x}{h3:08x}{h4:08x}")
}
