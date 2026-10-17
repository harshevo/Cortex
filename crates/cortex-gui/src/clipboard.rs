//! What a paste brings: files copied in the file manager, a picture, or
//! text. The window library only passes on text, so the rest is read here.

use std::path::PathBuf;

pub enum Pasted {
    Files(Vec<PathBuf>),

    /// A picture, as RGBA pixels.
    Picture {
        width: usize,
        height: usize,
        rgba: Vec<u8>,
    },

    Text(String),
}

/// What is on the clipboard, if anything can be pasted.
///
/// Files come first: copying one in Finder also puts its name on the
/// clipboard as text. Text comes before a picture, because word processors
/// put a picture of the copied text beside it.
pub fn read() -> Option<Pasted> {
    let mut clipboard = arboard::Clipboard::new().ok()?;

    if let Ok(files) = clipboard.get().file_list()
        && !files.is_empty()
    {
        return Some(Pasted::Files(files));
    }

    if let Ok(text) = clipboard.get_text()
        && !text.is_empty()
    {
        return Some(Pasted::Text(text.replace("\r\n", "\n")));
    }

    let picture = clipboard.get_image().ok()?;

    Some(Pasted::Picture {
        width: picture.width,
        height: picture.height,
        rgba: picture.bytes.into_owned(),
    })
}
