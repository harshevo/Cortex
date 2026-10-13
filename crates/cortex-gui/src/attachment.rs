//! Pictures attached to an instruction: read from a file, made small
//! enough to send, and drawn.

use std::io::Cursor;
use std::path::Path;

use anyhow::{Context, Result};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use cortex_core::Image;
use eframe::egui;
use image::{DynamicImage, ImageFormat};

/// The file kinds offered when choosing a picture.
pub const EXTENSIONS: [&str; 5] = ["png", "jpg", "jpeg", "webp", "gif"];

/// Longest side sent to a model. Providers scale anything larger down
/// themselves, so more only costs upload time.
const MAX_EDGE: u32 = 1568;

/// Largest file sent as it is. Anything bigger is encoded again.
const MAX_BYTES: usize = 3_500_000;

/// Longest side of the copy kept for drawing.
const THUMBNAIL_EDGE: u32 = 640;

/// A picture waiting in the composer.
pub struct Attachment {
    /// The file's name, for the tooltip.
    pub name: String,

    pub image: Image,
}

pub fn is_picture(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| EXTENSIONS.contains(&extension.to_lowercase().as_str()))
}

/// Reads the picture in `path` and gets it ready to send.
pub fn load(path: &Path) -> Result<Attachment> {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());

    let bytes = std::fs::read(path).with_context(|| format!("cannot read {name}"))?;

    let format = image::guess_format(&bytes).with_context(|| format!("{name} is not a picture"))?;

    let decoded = image::load_from_memory(&bytes).with_context(|| format!("cannot read {name}"))?;

    let fits = decoded.width().max(decoded.height()) <= MAX_EDGE && bytes.len() <= MAX_BYTES;

    let (media_type, bytes) = match format {
        ImageFormat::Png if fits => ("image/png", bytes),
        ImageFormat::Jpeg if fits => ("image/jpeg", bytes),
        _ => encode(decoded)?,
    };

    Ok(Attachment {
        name,
        image: Image {
            media_type: media_type.to_string(),
            data: STANDARD.encode(bytes),
        },
    })
}

/// Scales `picture` down to what is sent at most and encodes it: as PNG
/// when it has transparency, otherwise as JPEG, which is far smaller for
/// photographs and screenshots.
fn encode(picture: DynamicImage) -> Result<(&'static str, Vec<u8>)> {
    let picture = if picture.width().max(picture.height()) > MAX_EDGE {
        picture.resize(MAX_EDGE, MAX_EDGE, image::imageops::FilterType::Triangle)
    } else {
        picture
    };

    let mut bytes = Cursor::new(Vec::new());

    if picture.color().has_alpha() {
        picture.write_to(&mut bytes, ImageFormat::Png)?;

        return Ok(("image/png", bytes.into_inner()));
    }

    DynamicImage::ImageRgb8(picture.to_rgb8()).write_to(&mut bytes, ImageFormat::Jpeg)?;

    Ok(("image/jpeg", bytes.into_inner()))
}

/// `image` as something to draw. It is decoded once and kept; `None` if
/// it cannot be decoded.
pub fn texture(context: &egui::Context, image: &Image) -> Option<egui::TextureHandle> {
    // Hashing megabytes every frame would be slow; the length and both
    // ends tell pictures apart well enough.
    let data = image.data.as_bytes();
    let edge = data.len().min(96);
    let id = egui::Id::new((
        "attachment",
        data.len(),
        &data[..edge],
        &data[data.len() - edge..],
    ));

    if let Some(kept) = context.data(|data| data.get_temp::<Option<egui::TextureHandle>>(id)) {
        return kept;
    }

    let texture = decode(image)
        .map(|pixels| context.load_texture("attachment", pixels, egui::TextureOptions::LINEAR));

    context.data_mut(|data| data.insert_temp(id, texture.clone()));

    texture
}

fn decode(image: &Image) -> Option<egui::ColorImage> {
    let bytes = STANDARD.decode(&image.data).ok()?;

    let picture = image::load_from_memory(&bytes)
        .ok()?
        .thumbnail(THUMBNAIL_EDGE, THUMBNAIL_EDGE)
        .to_rgba8();

    let size = [picture.width() as usize, picture.height() as usize];

    Some(egui::ColorImage::from_rgba_unmultiplied(size, &picture))
}

/// Draws `texture` filling `rect`, cut to the rectangle's shape from the
/// middle instead of stretched.
pub fn paint_cover(ui: &egui::Ui, rect: egui::Rect, texture: &egui::TextureHandle, radius: u8) {
    let size = texture.size_vec2();
    let wanted = rect.width() / rect.height();
    let actual = size.x / size.y;

    let (width, height) = if actual > wanted {
        (wanted / actual, 1.0)
    } else {
        (1.0, actual / wanted)
    };

    let uv = egui::Rect::from_center_size(egui::pos2(0.5, 0.5), egui::vec2(width, height));

    egui::Image::from_texture(egui::load::SizedTexture::new(texture.id(), rect.size()))
        .uv(uv)
        .corner_radius(egui::CornerRadius::same(radius))
        .paint_at(ui, rect);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_large_picture_is_scaled_down_and_still_decodes() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("wide.png");

        image::RgbImage::from_pixel(3000, 1000, image::Rgb([200, 40, 40]))
            .save(&path)
            .unwrap();

        let attachment = load(&path).unwrap();

        assert_eq!(attachment.name, "wide.png");
        assert_eq!(attachment.image.media_type, "image/jpeg");

        let bytes = STANDARD.decode(&attachment.image.data).unwrap();
        let sent = image::load_from_memory(&bytes).unwrap();

        assert_eq!(sent.width(), MAX_EDGE);
        assert!(decode(&attachment.image).is_some());
    }

    #[test]
    fn a_small_png_is_sent_as_it_is() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("dot.png");

        image::RgbaImage::from_pixel(4, 4, image::Rgba([0, 0, 0, 0]))
            .save(&path)
            .unwrap();

        let attachment = load(&path).unwrap();

        assert_eq!(attachment.image.media_type, "image/png");
        assert_eq!(
            STANDARD.decode(&attachment.image.data).unwrap(),
            std::fs::read(&path).unwrap()
        );
    }

    #[test]
    fn a_text_file_is_refused() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("notes.png");

        std::fs::write(&path, "not a picture").unwrap();

        assert!(load(&path).is_err());
    }
}
