//! Strict JSONL framing, as pi's RPC documentation requires.
//!
//! Records are split only on LF (`\n`), an optional preceding CR is stripped, and
//! nothing else is treated as a boundary. Splitting on raw bytes is what makes this
//! correct: U+2028/U+2029 are multi-byte UTF-8 sequences that never contain `0x0A`.

use std::io::{self, BufRead, Write};

use serde::Serialize;

/// Reads LF-terminated records from a byte stream.
pub struct JsonlReader<R> {
    inner: R,
    buffer: Vec<u8>,
}

impl<R: BufRead> JsonlReader<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            buffer: Vec::with_capacity(8 * 1024),
        }
    }

    /// Returns the next non-empty record without its line terminator, or `None` at EOF.
    ///
    /// A final record without a trailing LF is still returned.
    pub fn next_record(&mut self) -> io::Result<Option<&[u8]>> {
        loop {
            self.buffer.clear();
            let read = self.inner.read_until(b'\n', &mut self.buffer)?;
            if read == 0 {
                return Ok(None);
            }
            let mut end = self.buffer.len();
            if self.buffer[..end].ends_with(b"\n") {
                end -= 1;
            }
            if self.buffer[..end].ends_with(b"\r") {
                end -= 1;
            }
            if self.buffer[..end].iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            return Ok(Some(&self.buffer[..end]));
        }
    }
}

/// Serializes `value` as one compact JSON record terminated by LF.
pub fn write_record<W: Write, T: Serialize>(writer: &mut W, value: &T) -> io::Result<()> {
    let mut line = serde_json::to_vec(value).map_err(io::Error::other)?;
    line.push(b'\n');
    writer.write_all(&line)?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn records(input: &[u8]) -> Vec<String> {
        let mut reader = JsonlReader::new(Cursor::new(input.to_vec()));
        let mut out = Vec::new();
        while let Some(record) = reader.next_record().unwrap() {
            out.push(String::from_utf8(record.to_vec()).unwrap());
        }
        out
    }

    #[test]
    fn splits_only_on_lf_and_strips_cr() {
        let input = "{\"a\":1}\r\n{\"b\":\"x\u{2028}y\u{2029}z\"}\n\n{\"c\":3}";
        assert_eq!(
            records(input.as_bytes()),
            vec!["{\"a\":1}", "{\"b\":\"x\u{2028}y\u{2029}z\"}", "{\"c\":3}"]
        );
    }

    #[test]
    fn write_record_appends_single_lf() {
        let mut out = Vec::new();
        write_record(&mut out, &serde_json::json!({"type":"abort"})).unwrap();
        assert_eq!(out, b"{\"type\":\"abort\"}\n");
    }
}
