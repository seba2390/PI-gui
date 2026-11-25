//! The built-in readers of Office, OpenDocument and EPUB files: their text as Markdown, with
//! headings, lists and tables.

mod book;
mod html;
mod markdown;
mod opendocument;
mod package;
mod sheets;
mod slides;
#[cfg(test)]
pub(crate) mod testing;
mod word;

use crate::Kind;

/// What password-protected files and those of old programs start with.
const COMPOUND: &[u8] = b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1";

/// The text of a file of `kind`, which is no PDF.
pub fn read(kind: Kind, bytes: Vec<u8>) -> Result<String, String> {
    if kind != Kind::Xls && bytes.starts_with(COMPOUND) {
        return Err("it is protected with a password, or of an older program".into());
    }
    match kind {
        Kind::Docx => word::read(bytes),
        Kind::Pptx => slides::read(bytes),
        Kind::Odt => opendocument::text(bytes),
        Kind::Odp => opendocument::slides(bytes),
        Kind::Xlsx | Kind::Xlsb | Kind::Xls | Kind::Ods => sheets::read(kind, bytes),
        Kind::Epub => book::read(bytes),
        Kind::Pdf => Err("it is not read like this".into()),
    }
}
