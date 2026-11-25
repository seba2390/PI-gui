//! PDF files for tests.

use std::fmt::Write as _;

/// The object of page `page`.
pub fn page_object(page: usize) -> usize {
    6 + page * 2
}

/// A file with `pages` pages of 100 × 200 points, each with a black square of 50 points at its top
/// left corner. The first has `annotations`. Names lead to: `chapter` (the third page), `nested`
/// and `old` (the second).
pub fn pdf(pages: usize, annotations: &str) -> Vec<u8> {
    let kids: Vec<_> = (0..pages)
        .map(|page| format!("{} 0 R", page_object(page)))
        .collect();
    let (second, third) = (page_object(1), page_object(2));
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R /Names << /Dests 3 0 R >> /Dests 4 0 R >>".to_owned(),
        format!(
            "<< /Type /Pages /Count {pages} /Kids [{}] /MediaBox [0 0 100 200] >>",
            kids.join(" ")
        ),
        format!(
            "<< /Kids [<< /Names [(chapter) [{third} 0 R /XYZ 0 120 0] \
             (nested) << /D [{second} 0 R /Fit] >>] >>] >>"
        ),
        format!("<< /old [{second} 0 R /Fit] >>"),
        "null".to_owned(),
    ];
    for page in 0..pages {
        let annotations = if page == 0 { annotations } else { "" };
        let content = page_object(page) + 1;
        objects.push(format!(
            "<< /Type /Page /Parent 2 0 R /Contents {content} 0 R /Annots [{annotations}] >>"
        ));
        let square = "0 g 0 150 50 50 re f";
        objects.push(format!(
            "<< /Length {} >>\nstream\n{square}\nendstream",
            square.len()
        ));
    }
    let mut out = String::from("%PDF-1.7\n");
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(out.len());
        let _ = write!(out, "{} 0 obj\n{object}\nendobj\n", index + 1);
    }
    let xref = out.len();
    let _ = write!(out, "xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1);
    for offset in offsets {
        let _ = writeln!(out, "{offset:010} 00000 n ");
    }
    let _ = write!(
        out,
        "trailer << /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF",
        objects.len() + 1
    );
    out.into_bytes()
}
