//! Spreadsheets.

use std::io::Cursor;

use calamine::{Data, Ods, Range, Reader, Xls, XlsError, Xlsb, Xlsx};

use super::markdown::Markdown;
use crate::Kind;

/// Of all sheets together; more is not read.
const MAX_CELLS: usize = 2_000_000;

pub fn read(kind: Kind, bytes: Vec<u8>) -> Result<String, String> {
    let file = Cursor::new(bytes);
    match kind {
        Kind::Xlsx => sheets(Xlsx::new(file).map_err(|_| damaged())?),
        Kind::Xlsb => sheets(Xlsb::new(file).map_err(|_| damaged())?),
        Kind::Ods => sheets(Ods::new(file).map_err(|error| match error {
            calamine::OdsError::Password => protected(),
            _ => damaged(),
        })?),
        _ => sheets(Xls::new(file).map_err(|error| match error {
            XlsError::Password => protected(),
            _ => damaged(),
        })?),
    }
}

fn damaged() -> String {
    "it is damaged, or not what its name says".into()
}

fn protected() -> String {
    "it is protected with a password".into()
}

fn sheets<R: Reader<Cursor<Vec<u8>>>>(mut book: R) -> Result<String, String> {
    let names = book.sheet_names();
    let mut out = Markdown::default();
    let mut left = MAX_CELLS;
    let mut written = false;
    for name in names {
        // Charts are sheets without cells.
        let Ok(range) = book.worksheet_range(&name) else {
            continue;
        };
        if range.is_empty() {
            continue;
        }
        out.heading(2, &name);
        sheet(&range, &mut left, &mut out);
        written = true;
    }
    if written {
        Ok(out.finish())
    } else {
        Err("there are no cells in it".into())
    }
}

fn sheet(range: &Range<Data>, left: &mut usize, out: &mut Markdown) {
    let mut rows = Vec::new();
    let mut read = 0;
    for row in range.rows() {
        if row.len() > *left {
            break;
        }
        *left -= row.len();
        read += 1;
        if row.iter().any(|cell| *cell != Data::Empty) {
            rows.push(row.iter().map(text).collect());
        }
    }
    // A column of values is not a frame around text.
    if rows.iter().all(|row: &Vec<String>| row.len() == 1) {
        for row in rows {
            out.item(0, None, &row[0]);
        }
    } else {
        out.table(&rows);
    }
    let missing = range.height() - read;
    if missing > 0 {
        out.paragraph(&format!("({missing} more rows are left out.)"));
    }
}

fn text(cell: &Data) -> String {
    match cell {
        Data::Empty => String::new(),
        Data::String(text) | Data::DateTimeIso(text) | Data::DurationIso(text) => text.clone(),
        Data::Int(number) => number.to_string(),
        Data::Float(number) => float(*number),
        Data::Bool(true) => "TRUE".into(),
        Data::Bool(false) => "FALSE".into(),
        Data::Error(error) => error.to_string(),
        Data::DateTime(time) if time.is_duration() => {
            let seconds = (time.as_f64() * 86_400.).round().abs();
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // a duration
            let seconds = seconds as u64;
            let (hours, rest) = (seconds / 3600, seconds % 3600);
            format!("{hours}:{:02}:{:02}", rest / 60, rest % 60)
        }
        Data::DateTime(time) => {
            let (year, month, day, hour, minute, second, _) = time.to_ymd_hms_milli();
            let date = format!("{year:04}-{month:02}-{day:02}");
            let clock = format!("{hour:02}:{minute:02}:{second:02}");
            if time.as_f64() < 1. {
                clock
            } else if (hour, minute, second) == (0, 0, 0) {
                date
            } else {
                format!("{date} {clock}")
            }
        }
    }
}

/// Without the noise of computing with halves, as the sheet shows it at most: 15 digits.
fn float(number: f64) -> String {
    if !number.is_finite() || number == 0. {
        return number.to_string();
    }
    let rounded: f64 = format!("{number:.14e}").parse().unwrap_or(number);
    rounded.to_string()
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::super::testing::zip;
    use super::*;

    fn book(rows: &str) -> Vec<u8> {
        let ns = "xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"";
        let rel = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
        let package = "http://schemas.openxmlformats.org/package/2006";
        let sheet = "application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml";
        let main = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml";
        zip(&[
            (
                "[Content_Types].xml",
                &format!(
                    "<Types xmlns=\"{package}/content-types\"><Override PartName=\"/xl/workbook.xml\" ContentType=\"{main}\"/>\
                     <Override PartName=\"/xl/worksheets/sheet1.xml\" ContentType=\"{sheet}\"/></Types>"
                ),
            ),
            (
                "_rels/.rels",
                &format!(
                    "<Relationships xmlns=\"{package}/relationships\"><Relationship Id=\"r1\" Type=\"{rel}/officeDocument\" Target=\"xl/workbook.xml\"/></Relationships>"
                ),
            ),
            (
                "xl/workbook.xml",
                &format!(
                    "<workbook {ns} xmlns:r=\"{rel}\"><sheets><sheet name=\"Båd\" sheetId=\"1\" r:id=\"r1\"/></sheets></workbook>"
                ),
            ),
            (
                "xl/_rels/workbook.xml.rels",
                &format!(
                    "<Relationships xmlns=\"{package}/relationships\"><Relationship Id=\"r1\" Type=\"{rel}/worksheet\" Target=\"worksheets/sheet1.xml\"/></Relationships>"
                ),
            ),
            (
                "xl/styles.xml",
                &format!(
                    "<styleSheet {ns}><cellXfs count=\"2\"><xf numFmtId=\"0\"/><xf numFmtId=\"14\"/></cellXfs></styleSheet>"
                ),
            ),
            (
                "xl/worksheets/sheet1.xml",
                &format!("<worksheet {ns}><sheetData>{rows}</sheetData></worksheet>"),
            ),
        ])
    }

    #[test]
    fn cells_as_they_are_shown() {
        let rows = "<row r=\"2\"><c r=\"B2\" t=\"inlineStr\"><is><t>Navn</t></is></c><c r=\"C2\" t=\"inlineStr\"><is><t>Pris</t></is></c><c r=\"D2\" t=\"inlineStr\"><is><t>Dato</t></is></c></row>\
            <row r=\"4\"><c r=\"B4\" t=\"inlineStr\"><is><t>a|b</t></is></c><c r=\"C4\"><v>400</v></c><c r=\"D4\" s=\"1\"><v>45000</v></c></row>\
            <row r=\"5\"><c r=\"C5\"><v>0.30000000000000004</v></c><c r=\"D5\" t=\"b\"><v>1</v></c></row>";
        assert_eq!(
            read(Kind::Xlsx, book(rows)).unwrap(),
            "## Båd\n\n| Navn | Pris | Dato |\n| --- | --- | --- |\n| a\\|b | 400 | 2023-03-15 |\n|  | 0.3 | TRUE |\n"
        );
        let column = "<row r=\"1\"><c r=\"A1\"><v>1</v></c></row><row r=\"2\"><c r=\"A2\"><v>2.5</v></c></row>";
        assert_eq!(
            read(Kind::Xlsx, book(column)).unwrap(),
            "## Båd\n\n- 1\n- 2.5\n"
        );
    }

    #[test]
    fn files_that_are_something_else() {
        assert!(read(Kind::Xlsx, book("")).unwrap_err().contains("no cells"));
        for kind in [Kind::Xlsx, Kind::Xlsb, Kind::Xls, Kind::Ods] {
            assert!(read(kind, b"no".to_vec()).is_err());
        }
    }
}
