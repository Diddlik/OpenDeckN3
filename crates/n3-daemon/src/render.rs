//! Image helpers: decoding plugin data URLs and producing UI previews.

use std::{io::Cursor, path::Path};

use anyhow::Context;
use base64::Engine;
use data_url::DataUrl;
use image::DynamicImage;

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
}
