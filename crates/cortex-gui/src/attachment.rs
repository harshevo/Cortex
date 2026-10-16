//! Pictures attached to an instruction: read from a file, made small
//! enough to send, and drawn.

use crate::background::Background;
use std::collections::VecDeque;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

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

#[derive(Default)]
pub struct Attachments {
    ready: Vec<Attachment>,
    waiting: VecDeque<PathBuf>,
    job: Background<Result<Attachment, String>>,
}

impl std::ops::Deref for Attachments {
    type Target = Vec<Attachment>;
    fn deref(&self) -> &Self::Target {
        &self.ready
    }
}
impl std::ops::DerefMut for Attachments {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.ready
    }
}
impl Attachments {
    pub fn loading(&self) -> bool {
        self.job.pending() || !self.waiting.is_empty()
    }
    pub fn enqueue(&mut self, paths: &[PathBuf]) -> Option<String> {
        let mut error = None;
        for path in paths {
            if !is_picture(path) {
                error = Some("Only pictures can be attached (PNG, JPEG, WebP, GIF).".into());
            } else if self.ready.len() + self.waiting.len() + usize::from(self.job.pending()) >= 16
            {
                error = Some("Attach up to 16 pictures at a time.".into());
            } else {
                self.waiting.push_back(path.clone());
            }
        }
        error
    }
    pub fn poll(&mut self, context: &egui::Context) -> Option<String> {
        let mut error = None;
        if let Some(result) = self.job.poll() {
            match result {
                Ok(attachment) => self.ready.push(attachment),
                Err(why) => error = Some(why),
            }
        }
        if !self.job.pending()
            && let Some(path) = self.waiting.front()
        {
            let path = path.clone();
            if self.job.start(context, move |_| {
                load(&path).map_err(|error| format!("{error:#}"))
            }) {
                self.waiting.pop_front();
            }
        }
        error
    }
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

    const MAX_SOURCE_BYTES: u64 = 64 * 1024 * 1024;
    let file = std::fs::File::open(path).with_context(|| format!("cannot read {name}"))?;
    let mut bytes = Vec::new();
    file.take(MAX_SOURCE_BYTES + 1).read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() as u64 <= MAX_SOURCE_BYTES,
        "{name} exceeds the 64 MB attachment limit"
    );

    let format = image::guess_format(&bytes).with_context(|| format!("{name} is not a picture"))?;

    let decoded = decode_picture(&bytes).with_context(|| format!("cannot read {name}"))?;

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
    // Area averaging downsamples directly, without a full-width float buffer.
    let picture = if picture.width().max(picture.height()) > MAX_EDGE {
        picture.thumbnail(MAX_EDGE, MAX_EDGE)
    } else {
        picture
    };

    let mut bytes = Cursor::new(Vec::new());

    if picture.color().has_alpha() {
        picture.write_to(&mut bytes, ImageFormat::Png)?;

        return Ok(("image/png", bytes.into_inner()));
    }

    DynamicImage::ImageRgb8(picture.into_rgb8()).write_to(&mut bytes, ImageFormat::Jpeg)?;

    Ok(("image/jpeg", bytes.into_inner()))
}

/// Most pictures kept decoded for drawing. Each is up to
/// [`THUMBNAIL_EDGE`] square, about 1.6 MB of texture.
const MAX_TEXTURES: usize = 48;
const MAX_TEXTURE_BYTES: usize = 24 * 1024 * 1024;

#[derive(Default)]
struct Texture {
    handle: Option<egui::TextureHandle>,
    job: Background<Option<egui::ColorImage>>,
    finished: bool,
}

#[derive(Default)]
struct Textures {
    kept: std::collections::HashMap<egui::Id, (Texture, u64)>,
    clock: u64,
}

impl Textures {
    fn texture(
        &mut self,
        context: &egui::Context,
        id: egui::Id,
        image: &Image,
    ) -> Option<egui::TextureHandle> {
        self.clock += 1;
        let (texture, used) = self.kept.entry(id).or_default();
        *used = self.clock;
        if let Some(pixels) = texture.job.poll() {
            texture.handle = pixels.map(|pixels| {
                context.load_texture("attachment", pixels, egui::TextureOptions::LINEAR)
            });
            texture.finished = true;
        }
        if !texture.finished && !texture.job.pending() {
            let image = image.clone();
            texture.job.start(context, move |_| decode(&image));
        }
        let result = texture.handle.clone();
        self.trim();
        result
    }

    fn trim(&mut self) {
        let mut bytes: usize = self
            .kept
            .values()
            .filter_map(|(texture, _)| texture.handle.as_ref())
            .map(egui::TextureHandle::byte_size)
            .sum();
        while self.kept.len() > MAX_TEXTURES || bytes > MAX_TEXTURE_BYTES {
            if let Some(oldest) = self
                .kept
                .iter()
                .min_by_key(|(_, (_, used))| *used)
                .map(|(id, _)| *id)
                && let Some((texture, _)) = self.kept.remove(&oldest)
            {
                bytes -= texture
                    .handle
                    .as_ref()
                    .map_or(0, egui::TextureHandle::byte_size);
            }
        }
    }
}

fn textures(context: &egui::Context) -> std::sync::Arc<std::sync::Mutex<Textures>> {
    context.data_mut(|data| {
        data.get_temp_mut_or_default::<std::sync::Arc<std::sync::Mutex<Textures>>>(egui::Id::new(
            "attachment textures",
        ))
        .clone()
    })
}

/// `image` as something to draw. It is decoded once and kept while it is
/// among the pictures drawn most recently; `None` if it cannot be decoded.
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

    let store = textures(context);
    let mut store = store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    store.texture(context, id, image)
}

fn decode_picture(bytes: &[u8]) -> image::ImageResult<DynamicImage> {
    let mut reader = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    reader.decode()
}

fn decode(image: &Image) -> Option<egui::ColorImage> {
    let bytes = STANDARD.decode(&image.data).ok()?;

    let picture = decode_picture(&bytes).ok()?;
    let picture = if picture.width().max(picture.height()) > THUMBNAIL_EDGE {
        picture.thumbnail(THUMBNAIL_EDGE, THUMBNAIL_EDGE)
    } else {
        picture
    }
    .into_rgba8();

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
    fn preparing_an_attachment_returns_before_decoding_and_preserves_the_result() {
        let context = egui::Context::default();
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("large.png");
        image::RgbImage::from_pixel(3000, 2000, image::Rgb([1, 2, 3]))
            .save(&path)
            .unwrap();
        let mut attachments = Attachments::default();
        assert!(attachments.enqueue(&[path]).is_none());
        assert!(attachments.poll(&context).is_none());
        assert!(attachments.loading());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while attachments.loading() {
            assert!(attachments.poll(&context).is_none());
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0].name, "large.png");
        assert!(decode(&attachments[0].image).is_some());
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
        assert_eq!(decode(&attachment.image).unwrap().size, [4, 4]);
    }

    #[test]
    fn only_the_pictures_drawn_most_recently_stay_decoded() {
        let context = egui::Context::default();

        let picture = |shade: u8| {
            let mut bytes = Cursor::new(Vec::new());

            image::RgbImage::from_pixel(8, 8, image::Rgb([shade, 0, 0]))
                .write_to(&mut bytes, ImageFormat::Png)
                .unwrap();

            Image {
                media_type: "image/png".to_string(),
                data: STANDARD.encode(bytes.into_inner()),
            }
        };

        let first = picture(0);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let kept = loop {
            if let Some(texture) = texture(&context, &first) {
                break texture;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(1));
        };

        for shade in 1..=MAX_TEXTURES as u8 + 10 {
            texture(&context, &picture(shade));

            // The first is drawn now and then, so it is never the oldest.
            if shade % 8 == 0 {
                assert_eq!(texture(&context, &first).unwrap().id(), kept.id());
            }
        }

        let store = textures(&context);
        let store = store.lock().unwrap();

        assert_eq!(store.kept.len(), MAX_TEXTURES);

        drop(store);

        assert_eq!(texture(&context, &first).unwrap().id(), kept.id());
    }

    #[test]
    fn a_text_file_is_refused() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("notes.png");

        std::fs::write(&path, "not a picture").unwrap();

        assert!(load(&path).is_err());
    }

    #[test]
    fn large_textures_obey_the_byte_budget_and_keep_the_most_recent() {
        let context = egui::Context::default();
        let mut store = Textures::default();
        for used in 0..30 {
            let handle = context.load_texture(
                "large thumbnail",
                egui::ColorImage::filled([640, 640], egui::Color32::WHITE),
                egui::TextureOptions::LINEAR,
            );
            store.kept.insert(
                egui::Id::new(used),
                (
                    Texture {
                        handle: Some(handle),
                        finished: true,
                        ..Default::default()
                    },
                    used,
                ),
            );
            store.trim();
        }
        let bytes: usize = store
            .kept
            .values()
            .map(|(t, _)| t.handle.as_ref().unwrap().byte_size())
            .sum();
        assert!(bytes <= MAX_TEXTURE_BYTES);
        assert!(store.kept.contains_key(&egui::Id::new(29_u64)));
        assert!(!store.kept.contains_key(&egui::Id::new(0_u64)));
    }
}
