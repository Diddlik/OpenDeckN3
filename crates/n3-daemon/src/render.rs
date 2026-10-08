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
