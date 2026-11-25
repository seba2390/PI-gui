//! Plain text files, in whatever encoding they are.

use chardetng::{EncodingDetector, Iso2022JpDetection, Utf8Detection};
use encoding_rs::{Encoding, UTF_16BE, UTF_16LE};

/// What tells a file's encoding: so much of its start.
const SAMPLE: usize = 64 * 1024;

/// The text of a file with the contents `bytes`, or `None` if they are no text.
pub fn decode(bytes: Vec<u8>) -> Option<String> {
    if let Some((encoding, mark)) = Encoding::for_bom(&bytes) {
        let (text, _) = encoding.decode_without_bom_handling(&bytes[mark..]);
        return Some(text.into_owned()).filter(|text| is_text(text));
    }
    let bytes = match String::from_utf8(bytes) {
        Ok(text) => return Some(text).filter(|text| is_text(text)),
        Err(error) => error.into_bytes(),
    };
    let sample = &bytes[..bytes.len().min(SAMPLE)];
    let encoding = if sample.contains(&0) {
        utf16(sample)?
    } else {
        // What has no NULs but other control characters is an archive or the like.
        let controls = sample.iter().filter(|byte| is_control(**byte)).count();
        if controls * 200 > sample.len() {
            return None;
        }
        let mut detector = EncodingDetector::new(Iso2022JpDetection::Deny);
        detector.feed(sample, true);
        detector.guess(None, Utf8Detection::Deny)
    };
    let (text, _) = encoding.decode_without_bom_handling(&bytes);
    Some(text.into_owned()).filter(|text| is_text(text))
}

fn is_control(byte: u8) -> bool {
    byte < 0x20 && !matches!(byte, b'\t' | b'\n' | b'\r' | 0x0c | 0x1b)
}

/// UTF-16 without a mark: text in it has many characters below 256, whose other byte is zero.
fn utf16(sample: &[u8]) -> Option<&'static Encoding> {
    let (pairs, _) = sample.as_chunks::<2>();
    let zeros = |at: usize| pairs.iter().filter(|pair| pair[at] == 0).count();
    let (first, second) = (zeros(0), zeros(1));
    if second > pairs.len() * 2 / 5 && first * 50 < second {
        Some(UTF_16LE)
    } else if first > pairs.len() * 2 / 5 && second * 50 < first {
        Some(UTF_16BE)
    } else {
        None
    }
}

/// Whether `text`, as it was decoded, makes sense.
fn is_text(text: &str) -> bool {
    let (mut wrong, mut count) = (0, 0);
    for c in text.chars().take(SAMPLE) {
        count += 1;
        wrong += usize::from(
            c == '\0' || c == char::REPLACEMENT_CHARACTER || (c.is_ascii() && is_control(c as u8)),
        );
    }
    wrong * 200 <= count
}

#[cfg(test)]
mod tests {
    use encoding_rs::{SHIFT_JIS, WINDOWS_1251, WINDOWS_1252};
    use pretty_assertions::assert_eq;

    use super::*;

    fn encoded(encoding: &'static Encoding, text: &str) -> Vec<u8> {
        encoding.encode(text).0.into_owned()
    }

    fn utf16_bytes(text: &str, little: bool) -> Vec<u8> {
        text.encode_utf16()
            .flat_map(|unit| {
                if little {
                    unit.to_le_bytes()
                } else {
                    unit.to_be_bytes()
                }
            })
            .collect()
    }

    #[test]
    fn utf_8_stays_as_it_is() {
        for text in [
            "",
            "plain\n",
            "Blåbærgrød på øen",
            "日本語",
            "\u{1b}[31mred\u{1b}[0m\t1\r\n",
        ] {
            assert_eq!(decode(text.into()).as_deref(), Some(text));
        }
        assert_eq!(decode(b"\xef\xbb\xbfhi".to_vec()).as_deref(), Some("hi"));
    }

    #[test]
    fn other_encodings_are_found_out() {
        let danish = "Blåbærgrød på øen koster 5 € om året, og det er ikke så dårligt endda.";
        assert_eq!(
            decode(encoded(WINDOWS_1252, danish)).as_deref(),
            Some(danish)
        );
        let russian = "Это обычный текст на русском языке, записанный в старой кодировке.";
        assert_eq!(
            decode(encoded(WINDOWS_1251, russian)).as_deref(),
            Some(russian)
        );
        let japanese = "これは日本語のテキストファイルです。漢字とカタカナも入っています。";
        assert_eq!(
            decode(encoded(SHIFT_JIS, japanese)).as_deref(),
            Some(japanese)
        );
        for little in [true, false] {
            assert_eq!(
                decode(utf16_bytes(danish, little)).as_deref(),
                Some(danish),
                "without a mark"
            );
            let mut marked = if little {
                vec![0xff, 0xfe]
            } else {
                vec![0xfe, 0xff]
            };
            marked.extend(utf16_bytes(japanese, little));
            assert_eq!(decode(marked).as_deref(), Some(japanese));
        }
    }

    #[test]
    fn other_files_are_no_text() {
        let noise: Vec<u8> = (0..4000_u32)
            .map(|n| u8::try_from(n.wrapping_mul(2_654_435_761) >> 24).unwrap())
            .collect();
        let zip = [b"PK\x03\x04\x14\x00\x08\x08\x08".as_slice(), &noise].concat();
        for bytes in [
            b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\xff".to_vec(),
            noise,
            zip,
            vec![0; 100],
            b"a\0b\0\0c\xff".to_vec(),
        ] {
            assert_eq!(decode(bytes), None);
        }
    }
}
