//! Le logo de BetterClaude : une pièce de puzzle couleur Claude, une étoile rayonnante
//! blanche au centre. Dessiné ici (pas de fichier image) : icône de la fenêtre, et icône de
//! l'exe, que `build.sh` y écrit avec `--embed-icon`.

use crate::button::puzzle_sdf;

const PIECE: [f32; 3] = [217.0, 119.0, 87.0];
const STAR: [f32; 3] = [255.0, 250.0, 243.0];

/// Distance à l'étoile : 12 rayons effilés autour du centre du corps de la pièce.
fn star_sdf(x: f32, y: f32) -> f32 {
    let (cx, cy) = (8.45, 15.55);
    let mut d = f32::MAX;
    for i in 0..12 {
        let angle = i as f32 * std::f32::consts::TAU / 12.0 + 0.13;
        let len = if i % 2 == 0 { 4.1 } else { 3.2 };
        let (ex, ey) = (cx + angle.cos() * len, cy + angle.sin() * len);
        // Distance au segment centre → bout, épaisseur qui diminue vers le bout.
        let (px, py, vx, vy) = (x - cx, y - cy, ex - cx, ey - cy);
        let h = ((px * vx + py * vy) / (vx * vx + vy * vy)).clamp(0.0, 1.0);
        let dist = ((px - vx * h).powi(2) + (py - vy * h).powi(2)).sqrt();
        d = d.min(dist - (0.62 - 0.42 * h));
    }
    d
}

/// Logo `size` × `size` en RGBA (non prémultiplié), suréchantillonné 4×4.
pub fn rgba(size: u32) -> Vec<u8> {
    let s = size as f32;
    // La pièce occupe la grille 24×24 de 2,95 à 21,05 : on l'agrandit à toute l'icône.
    let (lo, span) = (2.6, 18.8);
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let (mut piece, mut star) = (0.0f32, 0.0f32);
            for sy in 0..4 {
                for sx in 0..4 {
                    let gx = lo + (x as f32 + (sx as f32 + 0.5) / 4.0) / s * span;
                    let gy = lo + (y as f32 + (sy as f32 + 0.5) / 4.0) / s * span;
                    if puzzle_sdf(gx, gy) <= 0.0 {
                        piece += 1.0;
                        if star_sdf(gx, gy) <= 0.0 {
                            star += 1.0;
                        }
                    }
                }
            }
            let (a, t) = (piece / 16.0, if piece > 0.0 { star / piece } else { 0.0 });
            for c in 0..3 {
                out.push((PIECE[c] * (1.0 - t) + STAR[c] * t).round() as u8);
            }
            out.push((a * 255.0).round() as u8);
        }
    }
    out
}

/// Aperçu PNG du logo (sans compression), pour le regarder sans compiler l'exe.
pub fn write_png(path: &std::path::Path, size: u32) -> std::io::Result<()> {
    fn crc32(data: &[u8]) -> u32 {
        let mut crc = !0u32;
        for b in data {
            crc ^= *b as u32;
            for _ in 0..8 {
                crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
            }
        }
        !crc
    }
    let pixels = rgba(size);
    let mut raw = Vec::new();
    for row in pixels.chunks((size * 4) as usize) {
        raw.push(0);
        raw.extend(row);
    }
    // zlib « stocké » : blocs de 65 535 octets au plus, puis Adler-32.
    let mut z = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65_535).collect();
    for (i, block) in blocks.iter().enumerate() {
        z.push((i + 1 == blocks.len()) as u8);
        z.extend((block.len() as u16).to_le_bytes());
        z.extend((!(block.len() as u16)).to_le_bytes());
        z.extend(*block);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for byte in &raw {
        a = (a + *byte as u32) % 65_521;
        b = (b + a) % 65_521;
    }
    z.extend(((b << 16) | a).to_be_bytes());
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut chunk = |kind: &[u8], data: &[u8]| {
        png.extend((data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend(data);
        png.extend(&body);
        png.extend(crc32(&body).to_be_bytes());
    };
    let mut header = Vec::new();
    header.extend(size.to_be_bytes());
    header.extend(size.to_be_bytes());
    header.extend([8, 6, 0, 0, 0]);
    chunk(b"IHDR", &header);
    chunk(b"IDAT", &z);
    chunk(b"IEND", &[]);
    std::fs::write(path, png)
}

const SIZES: [u32; 6] = [16, 24, 32, 48, 64, 256];

/// Image d'icône Windows (DIB 32 bits, de bas en haut, suivie du masque) d'un logo.
fn dib(size: u32) -> Vec<u8> {
    let pixels = rgba(size);
    let mut out = Vec::new();
    for v in [40u32, size, size * 2] {
        out.extend(v.to_le_bytes());
    }
    out.extend(1u16.to_le_bytes());
    out.extend(32u16.to_le_bytes());
    out.extend([0u8; 24]);
    for y in (0..size).rev() {
        for x in 0..size {
            let i = ((y * size + x) * 4) as usize;
            out.extend([pixels[i + 2], pixels[i + 1], pixels[i], pixels[i + 3]]);
        }
    }
    out.extend(vec![0u8; ((size + 31) / 32 * 4 * size) as usize]);
    out
}

/// Écrit le logo comme icône de l'exe `exe` (ressources RT_ICON et RT_GROUP_ICON n° 1).
pub fn embed(exe: &std::path::Path) -> Result<(), String> {
    use windows_sys::Win32::System::LibraryLoader::{BeginUpdateResourceW, EndUpdateResourceW, UpdateResourceW};
    let path: Vec<u16> = exe.as_os_str().to_string_lossy().encode_utf16().chain(std::iter::once(0)).collect();
    const RT_ICON: u16 = 3;
    const RT_GROUP_ICON: u16 = 14;
    unsafe {
        let h = BeginUpdateResourceW(path.as_ptr(), 0);
        if h.is_null() {
            return Err(format!("{} : ouverture impossible.", exe.display()));
        }
        let mut group = vec![0u8, 0, 1, 0];
        group.extend((SIZES.len() as u16).to_le_bytes());
        for (i, size) in SIZES.iter().enumerate() {
            let data = dib(*size);
            let id = i as u16 + 1;
            UpdateResourceW(h, RT_ICON as usize as _, id as usize as _, 0, data.as_ptr() as _, data.len() as u32);
            let side = if *size >= 256 { 0 } else { *size as u8 };
            group.extend([side, side, 0, 0]);
            group.extend(1u16.to_le_bytes());
            group.extend(32u16.to_le_bytes());
            group.extend((data.len() as u32).to_le_bytes());
            group.extend(id.to_le_bytes());
        }
        UpdateResourceW(h, RT_GROUP_ICON as usize as _, 1usize as _, 0, group.as_ptr() as _, group.len() as u32);
        if EndUpdateResourceW(h, 0) == 0 {
            return Err(format!("{} : écriture de l'icône impossible.", exe.display()));
        }
    }
    Ok(())
}
