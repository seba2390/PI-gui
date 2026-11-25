//! Files for tests.

use std::io::{Cursor, Write as _};

use zip::ZipWriter;
use zip::write::SimpleFileOptions;

pub fn zip(parts: &[(&str, &str)]) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    for (name, contents) in parts {
        zip.start_file(*name, SimpleFileOptions::default()).unwrap();
        zip.write_all(contents.as_bytes()).unwrap();
    }
    zip.finish().unwrap().into_inner()
}
