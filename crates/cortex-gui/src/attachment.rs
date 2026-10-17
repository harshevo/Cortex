//! What is attached to an instruction: pictures, read from a file or the
//! clipboard and made small enough to send, and documents, whose text is
//! sent with the instruction. Both are drawn in the composer.

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

/// The picture kinds that are sent as pictures; any other file is read
/// as a document.
pub const EXTENSIONS: [&str; 5] = ["png", "jpg", "jpeg", "webp", "gif"];

/// Longest side sent to a model. Providers scale anything larger down
/// themselves, so more only costs upload time.
const MAX_EDGE: u32 = 1568;

/// Largest file sent as it is. Anything bigger is encoded again.
const MAX_BYTES: usize = 3_500_000;

/// Longest side of the copy kept for drawing.
const THUMBNAIL_EDGE: u32 = 640;

/// Most attachments waiting at once.
const MAX_ATTACHMENTS: usize = 16;

/// Largest document whose text is sent. The model's context is the real
/// limit; this keeps one paste from filling it.
const MAX_DOCUMENT_BYTES: u64 = 512 * 1024;

/// Opens and closes the text of a document in the instruction sent.
const DOCUMENT_OPEN: &str = "<attached_file name=\"";
const DOCUMENT_CLOSE: &str = "</attached_file>";

/// Something waiting in the composer.
pub struct Attachment {
    /// The file's name, for the tooltip.
    pub name: String,

    pub content: Content,
}

pub enum Content {
    Picture(Image),

    /// The text of a document.
    Document(String),
}

/// Where an attachment comes from.
pub enum Source {
    File(PathBuf),

    /// A picture on the clipboard, as RGBA pixels.
    Pixels {
        width: usize,
        height: usize,
        rgba: Vec<u8>,
    },
}

#[derive(Default)]
pub struct Attachments {
    ready: Vec<Attachment>,
    waiting: VecDeque<Source>,
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

    fn count(&self) -> usize {
        self.ready.len() + self.waiting.len() + usize::from(self.job.pending())
    }

    pub fn enqueue(&mut self, paths: &[PathBuf]) -> Option<String> {
        let mut error = None;
        for path in paths {
            if path.is_dir() {
                error = Some(format!(
                    "{} is a folder; attach files instead.",
                    file_name(path)
                ));
            } else if self.count() >= MAX_ATTACHMENTS {
                error = Some(format!("Attach up to {MAX_ATTACHMENTS} files at a time."));
            } else {
                self.waiting.push_back(Source::File(path.clone()));
            }
        }
        error
    }

    /// Attaches a picture taken from the clipboard.
    pub fn enqueue_pixels(&mut self, width: usize, height: usize, rgba: Vec<u8>) -> Option<String> {
        if self.count() >= MAX_ATTACHMENTS {
            return Some(format!("Attach up to {MAX_ATTACHMENTS} files at a time."));
        }
        self.waiting.push_back(Source::Pixels {
            width,
            height,
            rgba,
        });
        None
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
            && let Some(source) = self.waiting.pop_front()
        {
            // The job is shared; a refused start puts the source back.
            let source = std::sync::Arc::new(std::sync::Mutex::new(Some(source)));
            let given = source.clone();
            let started = self.job.start(context, move |_| {
                let source = given.lock().ok().and_then(|mut source| source.take());
                match source {
                    Some(source) => load_source(source).map_err(|error| format!("{error:#}")),
                    None => Err("the attachment was lost".to_string()),
                }
            });
            if !started
                && let Some(source) = source.lock().ok().and_then(|mut source| source.take())
            {
                self.waiting.push_front(source);
            }
        }
        error
    }

    /// Takes everything attached, for sending with `instruction`: the
    /// instruction with each document's text after it, and the pictures.
    pub fn take_message(&mut self, instruction: &str) -> (String, Vec<Image>) {
        compose(instruction, self.ready.drain(..))
    }
}

/// `instruction` with the text of each document after it, and the pictures.
pub fn compose(
    instruction: &str,
    attachments: impl IntoIterator<Item = Attachment>,
) -> (String, Vec<Image>) {
    let mut text = instruction.to_string();
    let mut images = Vec::new();

    for attachment in attachments {
        match attachment.content {
            Content::Picture(image) => images.push(image),
            Content::Document(body) => {
                let name = attachment.name.replace('"', "'");
                text.push_str(&format!(
                    "\n\n{DOCUMENT_OPEN}{name}\">\n{}\n{DOCUMENT_CLOSE}",
                    body.trim_end()
                ));
            }
        }
    }

    (text, images)
}

/// An instruction as it is shown: each attached document's text is
/// replaced by its name.
pub fn shown(instruction: &str) -> String {
    let mut shown = String::new();
    let mut rest = instruction;

    while let Some(start) = rest.find(DOCUMENT_OPEN) {
        let after = &rest[start + DOCUMENT_OPEN.len()..];
        let (Some(quote), Some(end)) = (after.find('"'), after.find(DOCUMENT_CLOSE)) else {
            break;
        };

        shown.push_str(rest[..start].trim_end_matches('\n'));
        shown.push_str(&format!("\n[Attached: {}]", &after[..quote]));

        rest = &after[end + DOCUMENT_CLOSE.len()..];
    }

    shown.push_str(rest);
    shown.trim_start_matches('\n').to_string()
}

pub fn is_picture(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| EXTENSIONS.contains(&extension.to_lowercase().as_str()))
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn load_source(source: Source) -> Result<Attachment> {
    match source {
        Source::File(path) if is_picture(&path) => load(&path),
        Source::File(path) => load_document(&path),
        Source::Pixels {
            width,
            height,
            rgba,
        } => load_pixels(width, height, rgba),
    }
}

/// Reads the text of the document in `path`. PDFs are turned into text,
/// and so are Word, RTF, OpenDocument and web pages on macOS; anything
/// else must be text.
pub fn load_document(path: &Path) -> Result<Attachment> {
    let name = file_name(path);

    let size = std::fs::metadata(path)
        .with_context(|| format!("cannot read {name}"))?
        .len();

    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_lowercase)
        .unwrap_or_default();

    let bytes = if extension == "pdf" {
        pdf_text(path, &name)?.into_bytes()
    } else if cfg!(target_os = "macos")
        && [
            "doc",
            "docx",
            "rtf",
            "rtfd",
            "odt",
            "wordml",
            "webarchive",
            "html",
            "htm",
        ]
        .contains(&extension.as_str())
    {
        convert_to_text(path).with_context(|| format!("cannot read the text of {name}"))?
    } else {
        anyhow::ensure!(
            size <= MAX_DOCUMENT_BYTES,
            "{name} is larger than the {} KB a document may be",
            MAX_DOCUMENT_BYTES / 1024
        );

        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .with_context(|| format!("cannot read {name}"))?
            .take(MAX_DOCUMENT_BYTES + 1)
            .read_to_end(&mut bytes)?;
        bytes
    };

    anyhow::ensure!(
        bytes.len() as u64 <= MAX_DOCUMENT_BYTES,
        "the text of {name} is longer than the {} KB a document may be",
        MAX_DOCUMENT_BYTES / 1024
    );

    let text = String::from_utf8(bytes)
        .ok()
        .filter(|text| !text.contains('\0'))
        .with_context(|| format!("{name} is not a picture or a text document"))?;

    Ok(Attachment {
        name,
        content: Content::Document(text),
    })
}

/// The text of the PDF in `path`, with runs of blank lines made one.
fn pdf_text(path: &Path, name: &str) -> Result<String> {
    /// Largest PDF read. Its text is capped later; this keeps a huge file
    /// from being read into memory first.
    const MAX_PDF_BYTES: u64 = 64 * 1024 * 1024;

    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .with_context(|| format!("cannot read {name}"))?
        .take(MAX_PDF_BYTES + 1)
        .read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() as u64 <= MAX_PDF_BYTES,
        "{name} exceeds the {} MB a PDF may be",
        MAX_PDF_BYTES / 1024 / 1024
    );

    // The reader panics on some malformed fonts; that is a file it
    // cannot read, not a reason to lose the attachment silently.
    let extracted = std::panic::catch_unwind(|| pdf_extract::extract_text_from_mem(&bytes))
        .map_err(|_| {
            anyhow::anyhow!("cannot read the text of {name}: the PDF is damaged or unsupported")
        })?;
    let extracted = extracted.map_err(|error| match error {
        pdf_extract::OutputError::PdfError(
            pdf_extract::Error::Decryption(_) | pdf_extract::Error::InvalidPassword,
        ) => anyhow::anyhow!("{name} is protected by a password; remove it and attach it again"),
        error => anyhow::anyhow!("cannot read the text of {name}: {error}"),
    })?;

    let mut text = String::with_capacity(extracted.len());
    let mut blank = true;
    for line in extracted.lines().map(str::trim_end) {
        if line.is_empty() {
            if !blank {
                text.push('\n');
            }
            blank = true;
        } else {
            text.push_str(line);
            text.push('\n');
            blank = false;
        }
    }

    anyhow::ensure!(
        !text.trim().is_empty(),
        "{name} has no text to read. It may be scanned pages; attach them as pictures instead."
    );

    Ok(text)
}

/// The text of a word-processor document or web page, by the system's
/// `textutil`.
fn convert_to_text(path: &Path) -> Result<Vec<u8>> {
    let output = std::process::Command::new("/usr/bin/textutil")
        .args(["-convert", "txt", "-stdout"])
        .arg(path)
        .output()?;

    anyhow::ensure!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr).trim()
    );

    Ok(output.stdout)
}

/// A picture from the clipboard, made ready to send.
fn load_pixels(width: usize, height: usize, rgba: Vec<u8>) -> Result<Attachment> {
    let picture = image::RgbaImage::from_raw(width as u32, height as u32, rgba)
        .context("the pasted picture is damaged")?;

    let picture = DynamicImage::ImageRgba8(picture);

    // Screenshots are opaque but come with an alpha channel; without one
    // they are sent as the far smaller JPEG.
    let opaque = picture
        .as_rgba8()
        .is_some_and(|pixels| pixels.pixels().all(|pixel| pixel[3] == 255));

    let picture = if opaque {
        DynamicImage::ImageRgb8(picture.into_rgb8())
    } else {
        picture
    };

    let (media_type, bytes) = encode(picture)?;

    let name = if media_type == "image/png" {
        "Pasted picture.png"
    } else {
        "Pasted picture.jpg"
    };

    Ok(Attachment {
        name: name.to_string(),
        content: Content::Picture(Image {
            media_type: media_type.to_string(),
            data: STANDARD.encode(bytes),
        }),
    })
}

/// Reads the picture in `path` and gets it ready to send.
pub fn load(path: &Path) -> Result<Attachment> {
    let name = file_name(path);

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
        content: Content::Picture(Image {
            media_type: media_type.to_string(),
            data: STANDARD.encode(bytes),
        }),
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

    fn picture(attachment: &Attachment) -> &Image {
        match &attachment.content {
            Content::Picture(image) => image,
            Content::Document(_) => panic!("{} is a document", attachment.name),
        }
    }

    #[test]
    fn a_text_document_is_attached_and_sent_with_the_instruction() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("notes.md");
        std::fs::write(&path, "# Notes\nremember the milk\n").unwrap();

        let attachment = load_source(Source::File(path)).unwrap();
        assert_eq!(attachment.name, "notes.md");

        let (sent, images) = compose("Summarise this", [attachment]);
        assert!(images.is_empty());
        assert!(sent.starts_with("Summarise this\n\n<attached_file name=\"notes.md\">\n# Notes"));
        assert!(sent.contains("remember the milk"));

        assert_eq!(shown(&sent), "Summarise this\n[Attached: notes.md]");
    }

    #[test]
    fn an_instruction_without_documents_is_shown_as_it_is() {
        assert_eq!(shown("fix the build\n\nplease"), "fix the build\n\nplease");
    }

    /// A one-page PDF showing `lines` in Helvetica.
    fn pdf(lines: &[&str]) -> Vec<u8> {
        let mut stream = String::from("BT /F1 12 Tf 72 720 Td 14 TL\n");
        for line in lines {
            stream.push_str(&format!("({line}) Tj T*\n"));
        }
        stream.push_str("ET");

        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>"
                .to_string(),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
            format!(
                "<< /Length {} >>\nstream\n{stream}\nendstream",
                stream.len()
            ),
        ];

        let mut bytes = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (number, object) in objects.iter().enumerate() {
            offsets.push(bytes.len());
            bytes.extend(format!("{} 0 obj\n{object}\nendobj\n", number + 1).bytes());
        }

        let xref = bytes.len();
        bytes.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).bytes());
        for offset in offsets {
            bytes.extend(format!("{offset:010} 00000 n \n").bytes());
        }
        bytes.extend(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objects.len() + 1
            )
            .bytes(),
        );
        bytes
    }

    #[test]
    fn a_pdf_is_attached_as_its_text() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("paper.pdf");
        std::fs::write(&path, pdf(&["Hello from a PDF", "second line"])).unwrap();

        let attachment = load_source(Source::File(path)).unwrap();
        assert_eq!(attachment.name, "paper.pdf");
        match &attachment.content {
            Content::Document(text) => {
                assert!(text.contains("Hello from a PDF"), "{text:?}");
                assert!(text.contains("second line"), "{text:?}");
            }
            Content::Picture(_) => panic!("a PDF became a picture"),
        }

        let (sent, images) = compose("Read this", [attachment]);
        assert!(images.is_empty());
        assert!(sent.contains("<attached_file name=\"paper.pdf\">"));
        assert_eq!(shown(&sent), "Read this\n[Attached: paper.pdf]");
    }

    #[test]
    fn binary_files_and_unreadable_pdfs_are_refused() {
        let folder = tempfile::tempdir().unwrap();

        let binary = folder.path().join("blob.bin");
        std::fs::write(&binary, [0_u8, 159, 146, 150, 0]).unwrap();
        assert!(load_source(Source::File(binary)).is_err());

        let damaged = folder.path().join("damaged.pdf");
        std::fs::write(&damaged, "%PDF-1.7").unwrap();
        let error = load_source(Source::File(damaged)).err().unwrap();
        assert!(format!("{error:#}").contains("damaged.pdf"));

        let scanned = folder.path().join("scanned.pdf");
        std::fs::write(&scanned, pdf(&[])).unwrap();
        let error = load_source(Source::File(scanned)).err().unwrap();
        assert!(format!("{error:#}").contains("no text"), "{error:#}");
    }

    #[test]
    fn a_folder_is_refused() {
        let folder = tempfile::tempdir().unwrap();
        let mut attachments = Attachments::default();

        assert!(
            attachments
                .enqueue(&[folder.path().to_path_buf()])
                .is_some()
        );
        assert!(!attachments.loading());
    }

    #[test]
    fn pasted_pixels_become_a_picture() {
        let opaque = load_pixels(3, 2, [10, 20, 30, 255].repeat(6)).unwrap();
        assert_eq!(picture(&opaque).media_type, "image/jpeg");

        let clear = load_pixels(3, 2, [10, 20, 30, 0].repeat(6)).unwrap();
        assert_eq!(picture(&clear).media_type, "image/png");
        assert!(decode(picture(&clear)).is_some());

        assert!(load_pixels(3, 2, vec![0; 5]).is_err());
    }

    #[test]
    fn a_large_picture_is_scaled_down_and_still_decodes() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("wide.png");

        image::RgbImage::from_pixel(3000, 1000, image::Rgb([200, 40, 40]))
            .save(&path)
            .unwrap();

        let attachment = load(&path).unwrap();

        assert_eq!(attachment.name, "wide.png");
        assert_eq!(picture(&attachment).media_type, "image/jpeg");

        let bytes = STANDARD.decode(&picture(&attachment).data).unwrap();
        let sent = image::load_from_memory(&bytes).unwrap();

        assert_eq!(sent.width(), MAX_EDGE);
        assert!(decode(picture(&attachment)).is_some());
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
        assert!(decode(picture(&attachments[0])).is_some());
    }

    #[test]
    fn a_small_png_is_sent_as_it_is() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("dot.png");

        image::RgbaImage::from_pixel(4, 4, image::Rgba([0, 0, 0, 0]))
            .save(&path)
            .unwrap();

        let attachment = load(&path).unwrap();

        assert_eq!(picture(&attachment).media_type, "image/png");
        assert_eq!(
            STANDARD.decode(&picture(&attachment).data).unwrap(),
            std::fs::read(&path).unwrap()
        );
        assert_eq!(decode(picture(&attachment)).unwrap().size, [4, 4]);
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

    #[test]
    fn zz_real_pdfs() {
        for path in std::env::var("REAL_PDFS").unwrap().split(':') {
            let started = std::time::Instant::now();
            match load_source(Source::File(path.into())) {
                Ok(a) => match a.content {
                    Content::Document(t) => eprintln!("OK {} {} bytes in {:?}\n---\n{}\n---", a.name, t.len(), started.elapsed(), &t[..t.len().min(600)]),
                    _ => unreachable!(),
                },
                Err(e) => eprintln!("ERR {path}: {e:#}"),
            }
        }
    }
}
