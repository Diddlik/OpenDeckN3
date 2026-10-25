//! Image helpers: decoding plugin data URLs, drawing key titles and
//! producing UI previews.

use std::{io::Cursor, path::Path, sync::LazyLock};

use ab_glyph::{Font, FontRef, PxScale, ScaleFont, point};
use anyhow::Context;
use base64::Engine;
use data_url::DataUrl;
use image::{DynamicImage, Rgba, RgbaImage, imageops::FilterType};

/// Key images are composed at this size; the driver scales to the device (64 px).
pub const KEY_SIZE: u32 = 144;
const KEY_BACKGROUND: Rgba<u8> = Rgba([0x12, 0x15, 0x19, 0xff]);

/// Geist SemiBold (SIL Open Font License, see assets/fonts/OFL.txt).
static FONT: LazyLock<FontRef<'static>> = LazyLock::new(|| {
    FontRef::try_from_slice(include_bytes!("../assets/fonts/Geist-SemiBold.ttf"))
        .expect("embedded font is valid")
});

fn text_width(text: &str, scale: PxScale) -> f32 {
    let font = FONT.as_scaled(scale);
    let mut width = 0.0;
    let mut previous = None;
    for c in text.chars() {
        let id = font.glyph_id(c);
        if let Some(p) = previous {
            width += font.kern(p, id);
        }
        width += font.h_advance(id);
        previous = Some(id);
    }
    width
}

fn draw_text(
    canvas: &mut RgbaImage,
    text: &str,
    scale: PxScale,
    x: f32,
    baseline: f32,
    color: [u8; 3],
    opacity: f32,
) {
    let font = FONT.as_scaled(scale);
    let mut caret = x;
    let mut previous = None;
    for c in text.chars() {
        let id = font.glyph_id(c);
        if let Some(p) = previous {
            caret += font.kern(p, id);
        }
        let glyph = id.with_scale_and_position(scale, point(caret, baseline));
        caret += font.h_advance(id);
        previous = Some(id);
        let Some(outline) = FONT.outline_glyph(glyph) else {
            continue;
        };
        let bounds = outline.px_bounds();
        outline.draw(|gx, gy, coverage| {
            let px = bounds.min.x as i32 + gx as i32;
            let py = bounds.min.y as i32 + gy as i32;
            if px < 0 || py < 0 || px >= canvas.width() as i32 || py >= canvas.height() as i32 {
                return;
            }
            let a = (coverage * opacity).clamp(0.0, 1.0);
            let pixel = canvas.get_pixel_mut(px as u32, py as u32);
            for (channel, value) in pixel.0.iter_mut().zip(color) {
                *channel = (*channel as f32 * (1.0 - a) + value as f32 * a).round() as u8;
            }
        });
    }
}

/// Picks the largest font size (down to a minimum) at which `text` fits,
/// shortening it with an ellipsis if necessary.
fn fit_text(text: &str, max_width: f32, max_size: f32, min_size: f32) -> (String, PxScale) {
    let mut size = max_size;
    while size > min_size && text_width(text, PxScale::from(size)) > max_width {
        size -= 1.0;
    }
    let scale = PxScale::from(size);
    if text_width(text, scale) <= max_width {
        return (text.to_owned(), scale);
    }
    let mut chars: Vec<char> = text.chars().collect();
    while !chars.is_empty() {
        chars.pop();
        let candidate: String = chars.iter().collect::<String>().trim_end().to_owned() + "…";
        if text_width(&candidate, scale) <= max_width {
            return (candidate, scale);
        }
    }
    ("…".to_owned(), scale)
}

/// Builds the final key image from an optional base image and title.
/// Without an image the title is centred on a dark background.
pub fn compose_key(base: Option<DynamicImage>, title: Option<&str>) -> Option<DynamicImage> {
    let title = title.map(str::trim).filter(|t| !t.is_empty());
    let mut canvas = match (&base, title) {
        (None, None) => return None,
        (Some(image), None) => return Some(image.clone()),
        (Some(image), Some(_)) => image
            .resize_exact(KEY_SIZE, KEY_SIZE, FilterType::Lanczos3)
            .to_rgba8(),
        (None, Some(_)) => RgbaImage::from_pixel(KEY_SIZE, KEY_SIZE, KEY_BACKGROUND),
    };
    let title = title.expect("handled above");
    let size = KEY_SIZE as f32;
    let max_width = size - 14.0;

    if base.is_some() {
        // Darken the bottom so the title stays readable on any image.
        let band = 56;
        for y in KEY_SIZE - band..KEY_SIZE {
            let t = (y - (KEY_SIZE - band)) as f32 / band as f32;
            let a = 0.72 * t.powf(0.8);
            for x in 0..KEY_SIZE {
                let p = canvas.get_pixel_mut(x, y);
                for c in &mut p.0[..3] {
                    *c = (*c as f32 * (1.0 - a)).round() as u8;
                }
            }
        }
        let (text, scale) = fit_text(title, max_width, 30.0, 20.0);
        let x = (size - text_width(&text, scale)) / 2.0;
        let baseline = size - 13.0;
        draw_text(
            &mut canvas,
            &text,
            scale,
            x,
            baseline + 2.0,
            [0, 0, 0],
            0.55,
        );
        draw_text(&mut canvas, &text, scale, x, baseline, [255, 255, 255], 1.0);
    } else {
        let (text, scale) = fit_text(title, max_width, 40.0, 20.0);
        let x = (size - text_width(&text, scale)) / 2.0;
        let ascent = FONT.as_scaled(scale).ascent();
        let baseline = (size + ascent * 0.72) / 2.0;
        draw_text(
            &mut canvas,
            &text,
            scale,
            x,
            baseline,
            [0xec, 0xee, 0xf1],
            1.0,
        );
    }
    Some(DynamicImage::ImageRgba8(canvas))
}

/// What the display keys show together while the deck is locked or the lock key is held.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LockScreen {
    Locked,
    /// `progress` runs from 0 to 1 towards locking (or unlocking).
    Holding {
        locking: bool,
        progress: f32,
        seconds_left: u32,
    },
}

const LOCK_ACCENT: [u8; 3] = [0x35, 0xe0, 0xd0];
const LOCK_LIGHT: [u8; 3] = [0xec, 0xee, 0xf1];
const LOCK_TRACK: [u8; 3] = [0x2a, 0x2f, 0x37];
const LOCK_DARK: [u8; 3] = [0x12, 0x15, 0x19];

/// Blends `color` where the signed distance `sdf` (pixels, negative inside) is below zero, anti-aliased.
fn fill(canvas: &mut RgbaImage, color: [u8; 3], sdf: impl Fn(f32, f32) -> f32) {
    for (x, y, pixel) in canvas.enumerate_pixels_mut() {
        let a = (0.5 - sdf(x as f32 + 0.5, y as f32 + 0.5)).clamp(0.0, 1.0);
        if a > 0.0 {
            for (channel, value) in pixel.0.iter_mut().zip(color) {
                *channel = (*channel as f32 * (1.0 - a) + value as f32 * a).round() as u8;
            }
        }
    }
}

fn rounded_box(x: f32, y: f32, center: (f32, f32), half: (f32, f32), radius: f32) -> f32 {
    let qx = (x - center.0).abs() - half.0 + radius;
    let qy = (y - center.1).abs() - half.1 + radius;
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - radius
}

/// Distance to a vertical segment from `(sx, top)` to `(sx, bottom)`.
fn vertical_segment(x: f32, y: f32, sx: f32, top: f32, bottom: f32) -> f32 {
    (x - sx).hypot(y - y.clamp(top, bottom))
}

/// Draws one lock picture across a `columns` × `rows` key grid and cuts it into key images, row-major.
pub fn lock_tiles(columns: u32, rows: u32, screen: LockScreen) -> Vec<DynamicImage> {
    let (w, h) = (columns * KEY_SIZE, rows * KEY_SIZE);
    let mut canvas = RgbaImage::from_pixel(w, h, KEY_BACKGROUND);
    // Proportions are designed for the 3 × 2 displays of the N3 (432 × 288 px).
    let s = (w as f32 / 432.0).min(h as f32 / 288.0);
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    let open_lift = 30.0 * s;
    let (lift, lock_color) = match screen {
        LockScreen::Locked => (0.0, LOCK_ACCENT),
        LockScreen::Holding {
            locking, progress, ..
        } => {
            let opened = if locking { 1.0 - progress } else { progress };
            (open_lift * opened.clamp(0.0, 1.0), LOCK_LIGHT)
        }
    };

    if let LockScreen::Holding { progress, .. } = screen {
        let (radius, half) = (118.0 * s, 5.0 * s);
        fill(&mut canvas, LOCK_TRACK, |x, y| {
            ((x - cx).hypot(y - cy) - radius).abs() - half
        });
        let progress = progress.clamp(0.0, 1.0);
        let end = progress * std::f32::consts::TAU;
        let cap = |angle: f32| (cx + radius * angle.sin(), cy - radius * angle.cos());
        let (start_cap, end_cap) = (cap(0.0), cap(end));
        fill(&mut canvas, LOCK_ACCENT, |x, y| {
            let (dx, dy) = (x - cx, y - cy);
            let on_arc = dx.atan2(-dy).rem_euclid(std::f32::consts::TAU) <= end;
            let ring = if on_arc {
                (dx.hypot(dy) - radius).abs() - half
            } else {
                f32::MAX
            };
            let caps = (x - start_cap.0)
                .hypot(y - start_cap.1)
                .min((x - end_cap.0).hypot(y - end_cap.1))
                - half;
            if progress > 0.0 {
                ring.min(caps)
            } else {
                f32::MAX
            }
        });
    }

    // Shackle: upper half ring with a short left leg, which leaves the body when lifted.
    let (sx, sy, sr, half) = (cx, cy - 18.0 * s - lift, 32.0 * s, 6.5 * s);
    fill(&mut canvas, lock_color, |x, y| {
        let arc = if y <= sy {
            ((x - sx).hypot(y - sy) - sr).abs()
        } else {
            f32::MAX
        };
        let left = vertical_segment(x, y, sx - sr, sy, sy + 10.0 * s);
        let right = vertical_segment(x, y, sx + sr, sy, sy + 44.0 * s);
        arc.min(left).min(right) - half
    });
    let body = (cx, cy + 24.0 * s);
    fill(&mut canvas, lock_color, |x, y| {
        rounded_box(x, y, body, (54.0 * s, 42.0 * s), 12.0 * s)
    });

    match screen {
        LockScreen::Locked => {
            let hole = (body.0, body.1 - 6.0 * s);
            fill(&mut canvas, LOCK_DARK, |x, y| {
                let round = (x - hole.0).hypot(y - hole.1) - 9.0 * s;
                let slot = rounded_box(
                    x,
                    y,
                    (hole.0, hole.1 + 12.0 * s),
                    (4.0 * s, 12.0 * s),
                    2.0 * s,
                );
                round.min(slot)
            });
        }
        LockScreen::Holding { seconds_left, .. } => {
            let text = seconds_left.to_string();
            let scale = PxScale::from(48.0 * s);
            let ascent = FONT.as_scaled(scale).ascent();
            let x = body.0 - text_width(&text, scale) / 2.0;
            draw_text(
                &mut canvas,
                &text,
                scale,
                x,
                body.1 + ascent * 0.36,
                LOCK_DARK,
                1.0,
            );
        }
    }

    (0..rows)
        .flat_map(|row| (0..columns).map(move |column| (column, row)))
        .map(|(column, row)| {
            let tile = image::imageops::crop_imm(
                &canvas,
                column * KEY_SIZE,
                row * KEY_SIZE,
                KEY_SIZE,
                KEY_SIZE,
            );
            DynamicImage::ImageRgba8(tile.to_image())
        })
        .collect()
}

/// Decodes `data:image/{png,jpeg,bmp};base64,...` into an image.
pub fn decode_data_url(url: &str) -> anyhow::Result<DynamicImage> {
    let url = DataUrl::process(url).map_err(|e| anyhow::anyhow!("invalid data url: {e:?}"))?;
    anyhow::ensure!(
        url.mime_type().type_ == "image",
        "not an image: {}",
        url.mime_type()
    );
    anyhow::ensure!(
        url.mime_type().subtype != "svg+xml",
        "SVG images are not supported yet"
    );
    let (body, _) = url
        .decode_to_vec()
        .map_err(|e| anyhow::anyhow!("invalid base64: {e:?}"))?;
    Ok(image::load_from_memory(&body)?)
}

/// Loads an action icon as referenced in a manifest. Stream Deck manifests
/// omit the extension, so `x`, `x.png` and `x@2x.png` are tried in order.
pub fn load_icon(plugin_dir: &Path, icon: &str) -> Option<DynamicImage> {
    if icon.is_empty() {
        return None;
    }
    let base = plugin_dir.join(icon);
    let candidates = [
        base.clone(),
        base.with_extension("png"),
        plugin_dir.join(format!("{icon}@2x.png")),
    ];
    candidates
        .iter()
        .filter(|p| p.is_file())
        .find_map(|p| image::open(p).ok())
}

/// Encodes an image as PNG data URL for the UI.
pub fn to_png_data_url(image: &DynamicImage) -> anyhow::Result<String> {
    let mut buf = Cursor::new(Vec::new());
    image
        .write_to(&mut buf, image::ImageFormat::Png)
        .context("encoding preview")?;
    Ok(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(buf.into_inner())
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_roundtrip_through_data_url() {
        let image = DynamicImage::new_rgb8(4, 4);
        let url = to_png_data_url(&image).unwrap();
        let back = decode_data_url(&url).unwrap();
        assert_eq!((back.width(), back.height()), (4, 4));
        assert!(decode_data_url("data:text/plain,hello").is_err());
    }

    #[test]
    fn composes_titles() {
        assert!(compose_key(None, None).is_none());
        assert!(compose_key(None, Some("  ")).is_none());

        let text_only = compose_key(None, Some("Ctrl+Shift+M")).unwrap().to_rgba8();
        assert_eq!(text_only.dimensions(), (KEY_SIZE, KEY_SIZE));
        let bright = text_only.pixels().filter(|p| p.0[0] > 200).count();
        assert!(bright > 50, "title should draw visible pixels");

        let red = DynamicImage::ImageRgba8(RgbaImage::from_pixel(64, 64, Rgba([255, 0, 0, 255])));
        let titled = compose_key(Some(red), Some("Größe → äöü"))
            .unwrap()
            .to_rgba8();
        assert_eq!(
            titled.get_pixel(72, 10).0,
            [255, 0, 0, 255],
            "top stays untouched"
        );
        assert!(titled.get_pixel(2, 142).0[0] < 200, "bottom is darkened");
    }

    #[test]
    fn lock_screen_spans_all_tiles() {
        let tiles = lock_tiles(3, 2, LockScreen::Locked);
        assert_eq!(tiles.len(), 6);
        assert!(
            tiles
                .iter()
                .all(|t| (t.width(), t.height()) == (KEY_SIZE, KEY_SIZE))
        );
        let accent = |t: &DynamicImage| {
            t.to_rgba8()
                .pixels()
                .filter(|p| p.0[..3] == LOCK_ACCENT)
                .count()
        };
        // The lock sits in the middle column, split between the two rows.
        assert!(accent(&tiles[1]) > 100 && accent(&tiles[4]) > 100);
        assert_eq!(accent(&tiles[0]), 0);

        let holding = |progress| LockScreen::Holding {
            locking: true,
            progress,
            seconds_left: 2,
        };
        let ring = |p| accent(&lock_tiles(3, 2, holding(p))[2]);
        assert!(ring(0.5) > ring(0.1), "progress ring grows clockwise");
    }

    #[test]
    fn long_titles_are_shortened() {
        let (text, scale) = fit_text(
            "Ein sehr langer Titel für eine kleine Taste",
            130.0,
            30.0,
            20.0,
        );
        assert!(text.ends_with('…'));
        assert!(text_width(&text, scale) <= 130.0);
        assert!(
            FONT.glyph_id('ä').0 != 0 && FONT.glyph_id('→').0 != 0,
            "font covers umlauts and arrows"
        );
    }
}
