//! The links on pages: to the web, and to other places in the file.

use std::cell::OnceCell;
use std::collections::HashMap;

use hayro::hayro_interpret::TransformExt as _;
use hayro::hayro_syntax::object::{Array, Dict, Name, Object, ObjectIdentifier};
use hayro::hayro_syntax::page::{Page, Pages};
use hayro::kurbo::{Point, Rect};

use super::{Link, Target};

/// A name tree is read this deep.
const DEPTH: usize = 32;
/// Of a page or a name tree, at most.
const MAX_LINKS: usize = 2000;
const MAX_NAMES: usize = 200_000;

pub struct Links<'a> {
    pages: &'a Pages<'a>,
    numbers: HashMap<ObjectIdentifier, usize>,
    /// Where names lead. Only files with such links need them.
    names: OnceCell<HashMap<Vec<u8>, Array<'a>>>,
}

impl<'a> Links<'a> {
    pub fn new(pages: &'a Pages<'a>) -> Self {
        let numbers = pages
            .iter()
            .enumerate()
            .filter_map(|(number, page)| Some((page.raw().obj_id()?, number)))
            .collect();
        Self {
            pages,
            numbers,
            names: OnceCell::new(),
        }
    }

    pub fn on(&self, number: usize) -> Vec<Link> {
        let Some(page) = self.pages.get(number) else {
            return Vec::new();
        };
        let Some(annotations) = page.raw().get::<Array<'a>>("Annots") else {
            return Vec::new();
        };
        let mut links = Vec::new();
        for annotation in annotations.iter::<Dict<'a>>() {
            let is_link = annotation
                .get::<Name<'a>>("Subtype")
                .is_some_and(|subtype| *subtype == *b"Link");
            if !is_link {
                continue;
            }
            if let Some(target) = self.target(&annotation, number) {
                for area in areas(&annotation, page) {
                    links.push(link(area, target.clone()));
                }
            }
            if links.len() >= MAX_LINKS {
                break;
            }
        }
        links
    }

    fn target(&self, annotation: &Dict<'a>, number: usize) -> Option<Target> {
        if let Some(destination) = annotation.get::<Object<'a>>("Dest") {
            return self.destination(destination);
        }
        let action = annotation.get::<Dict<'a>>("A")?;
        let kind = action.get::<Name<'a>>("S")?;
        match &*kind {
            b"URI" => match action.get::<Object<'a>>("URI")? {
                Object::String(uri) => web(&uri).map(Target::Uri),
                _ => None,
            },
            b"GoTo" => self.destination(action.get::<Object<'a>>("D")?),
            b"Named" => {
                let last = self.pages.len().checked_sub(1)?;
                let page = match &*action.get::<Name<'a>>("N")? {
                    b"NextPage" => number + 1,
                    b"PrevPage" => number.checked_sub(1)?,
                    b"FirstPage" => 0,
                    b"LastPage" => last,
                    _ => return None,
                };
                (page <= last).then_some(Target::Page { page, y: 0. })
            }
            _ => None,
        }
    }

    fn destination(&self, destination: Object<'a>) -> Option<Target> {
        match destination {
            Object::Array(array) => self.place(&array),
            Object::Dict(dict) => self.place(&dict.get::<Array<'a>>("D")?),
            Object::String(name) => self.place(self.names().get(&*name)?),
            Object::Name(name) => self.place(self.names().get(&*name)?),
            _ => None,
        }
    }

    /// `[page /XYZ left top zoom]` and the like.
    fn place(&self, array: &Array<'a>) -> Option<Target> {
        let number = match array.raw_iter().next()?.as_obj_ref() {
            Some(page) => *self.numbers.get(&page.into())?,
            None => usize::try_from(array.iter::<Object<'a>>().next()?.into_i32()?).ok()?,
        };
        let page = self.pages.get(number)?;
        let rest: Vec<Object<'a>> = array.iter::<Object<'a>>().skip(1).take(5).collect();
        let coordinate = |index: usize| rest.get(index).and_then(|o| o.clone().into_f32());
        let crop = page.intersected_crop_box();
        let (left, top) = match rest.first() {
            Some(Object::Name(kind)) => match &**kind {
                b"XYZ" => (coordinate(1), coordinate(2)),
                b"FitH" | b"FitBH" => (None, coordinate(1)),
                b"FitR" => (coordinate(1), coordinate(4)),
                _ => (None, None),
            },
            _ => (None, None),
        };
        let y = match top {
            Some(top) => {
                let left = left.map_or(crop.x0, f64::from);
                on_page(page, Point::new(left, f64::from(top))).y
            }
            None => 0.,
        };
        let height = f64::from(page.render_dimensions().1);
        Some(Target::Page {
            page: number,
            y: narrow(y.clamp(0., height)),
        })
    }

    fn names(&self) -> &HashMap<Vec<u8>, Array<'a>> {
        self.names.get_or_init(|| {
            let mut names = HashMap::new();
            let xref = self.pages.xref();
            let Some(catalog) = xref.get::<Dict<'a>>(xref.root_id()) else {
                return names;
            };
            if let Some(dict) = catalog.get::<Dict<'a>>("Dests") {
                for name in dict.keys() {
                    if let Some(object) = dict.get::<Object<'a>>(&*name) {
                        add(&mut names, &name, object);
                    }
                }
            }
            let tree = catalog
                .get::<Dict<'a>>("Names")
                .and_then(|all| all.get::<Dict<'a>>("Dests"));
            if let Some(tree) = tree {
                read_tree(&tree, DEPTH, &mut names);
            }
            names
        })
    }
}

fn read_tree<'a>(node: &Dict<'a>, depth: usize, names: &mut HashMap<Vec<u8>, Array<'a>>) {
    if let Some(pairs) = node.get::<Array<'a>>("Names") {
        let mut pairs = pairs.iter::<Object<'a>>();
        while let (Some(name), Some(object)) = (pairs.next(), pairs.next()) {
            if let Object::String(name) = name {
                add(names, &name, object);
            }
        }
    }
    if let Some(kids) = node.get::<Array<'a>>("Kids")
        && depth > 0
    {
        for kid in kids.iter::<Dict<'a>>() {
            if names.len() >= MAX_NAMES {
                return;
            }
            read_tree(&kid, depth - 1, names);
        }
    }
}

fn add<'a>(names: &mut HashMap<Vec<u8>, Array<'a>>, name: &[u8], object: Object<'a>) {
    let place = match object {
        Object::Array(array) => Some(array),
        Object::Dict(dict) => dict.get::<Array<'a>>("D"),
        _ => None,
    };
    if let Some(place) = place {
        names.insert(name.to_vec(), place);
    }
}

/// An address that may be opened. Others could start programs.
fn web(uri: &[u8]) -> Option<String> {
    let uri = std::str::from_utf8(uri).ok()?.trim();
    if uri.is_empty() || uri.chars().any(char::is_control) {
        return None;
    }
    let lower = uri.to_ascii_lowercase();
    if ["http://", "https://", "mailto:"]
        .iter()
        .any(|scheme| lower.starts_with(scheme))
    {
        return Some(uri.to_owned());
    }
    lower.starts_with("www.").then(|| format!("https://{uri}"))
}

/// Where an annotation is on its page: of a link over several lines, each line.
fn areas(annotation: &Dict<'_>, page: &Page<'_>) -> Vec<Rect> {
    let numbers = |key: &str| -> Vec<f64> {
        annotation
            .get::<Array<'_>>(key)
            .map_or_default(|array| array.iter::<f32>().map(f64::from).collect())
    };
    let quads = numbers("QuadPoints");
    let areas: Vec<Rect> = quads
        .as_chunks::<8>()
        .0
        .iter()
        .filter_map(|quad| around(quad, page))
        .collect();
    if areas.is_empty() {
        around(&numbers("Rect"), page).into_iter().collect()
    } else {
        areas
    }
}

/// The area around points (`x y x y…`) of a page, from its top left corner.
fn around(points: &[f64], page: &Page<'_>) -> Option<Rect> {
    let mut area: Option<Rect> = None;
    for point in points.as_chunks::<2>().0 {
        let point = on_page(page, Point::new(point[0], point[1]));
        area = Some(match area {
            Some(area) => area.union_pt(point),
            None => Rect::from_points(point, point),
        });
    }
    area.filter(|area| area.width() > 0. && area.height() > 0.)
}

fn on_page(page: &Page<'_>, point: Point) -> Point {
    page.initial_transform(true).to_kurbo() * point
}

fn link(area: Rect, target: Target) -> Link {
    Link {
        x: narrow(area.x0),
        y: narrow(area.y0),
        width: narrow(area.width()),
        height: narrow(area.height()),
        target,
    }
}

#[allow(clippy::cast_possible_truncation)] // points on a page
fn narrow(value: f64) -> f32 {
    value as f32
}

#[cfg(test)]
mod tests {
    use hayro::hayro_syntax::Pdf;
    use pretty_assertions::assert_eq;

    use super::super::testing;
    use super::*;

    fn links(bytes: Vec<u8>, page: usize) -> Vec<Link> {
        let pdf = Pdf::new(bytes).unwrap();
        Links::new(pdf.pages()).on(page)
    }

    fn targets(annotations: &str) -> Vec<Target> {
        let links = links(testing::pdf(3, annotations), 0);
        links.into_iter().map(|link| link.target).collect()
    }

    fn annotation(what: &str) -> String {
        format!("<< /Type /Annot /Subtype /Link /Rect [10 170 60 190] {what} >>")
    }

    #[test]
    fn links_to_the_web() {
        let uri = |uri: &str| annotation(&format!("/A << /S /URI /URI ({uri}) >>"));
        assert_eq!(
            links(testing::pdf(1, &uri("https://pi.dev/a?b=c")), 0),
            [Link {
                x: 10.,
                y: 10.,
                width: 50.,
                height: 20.,
                target: Target::Uri("https://pi.dev/a?b=c".into()),
            }]
        );
        assert_eq!(
            targets(&uri("www.pi.dev")),
            [Target::Uri("https://www.pi.dev".into())]
        );
        assert_eq!(targets(&uri("mailto:a@b.c")).len(), 1);
        for other in [
            "file:///etc/passwd",
            "javascript:x",
            "x-apple:y",
            "",
            "a.pdf",
        ] {
            assert_eq!(targets(&uri(other)), [], "{other}");
        }
        assert_eq!(targets(&annotation("/A << /S /Launch /F (a.app) >>")), []);
        let note =
            "<< /Type /Annot /Subtype /Text /Rect [0 0 9 9] /A << /S /URI /URI (https://a.b) >> >>";
        assert_eq!(targets(note), []);
    }

    /// Each line of a link is one area.
    #[test]
    fn links_over_lines() {
        let quads = "/QuadPoints [10 190 60 190 10 170 60 170 10 160 30 160 10 150 30 150]";
        let what = format!("{quads} /A << /S /URI /URI (https://a.b) >>");
        let areas: Vec<_> = links(testing::pdf(1, &annotation(&what)), 0)
            .iter()
            .map(|link| (link.x, link.y, link.width, link.height))
            .collect();
        assert_eq!(areas, [(10., 10., 50., 20.), (10., 40., 20., 10.)]);
    }

    #[test]
    fn links_within_the_file() {
        let page = |page, y| [Target::Page { page, y }];
        let second = testing::page_object(1);
        for (what, target) in [
            (
                format!("/Dest [{second} 0 R /XYZ 0 150 null]"),
                page(1, 50.),
            ),
            (format!("/Dest [{second} 0 R /FitH 180]"), page(1, 20.)),
            (format!("/Dest [{second} 0 R /Fit]"), page(1, 0.)),
            (
                format!("/A << /S /GoTo /D [{second} 0 R /XYZ null 900 0] >>"),
                page(1, 0.),
            ),
            ("/Dest [2 /XYZ 0 100 0]".into(), page(2, 100.)),
            ("/Dest (chapter)".into(), page(2, 80.)),
            ("/A << /S /GoTo /D (nested) >>".into(), page(1, 0.)),
            ("/Dest /old".into(), page(1, 0.)),
            ("/A << /S /Named /N /NextPage >>".into(), page(1, 0.)),
            ("/A << /S /Named /N /LastPage >>".into(), page(2, 0.)),
        ] {
            assert_eq!(targets(&annotation(&what)), target, "{what}");
        }
        for nowhere in ["/Dest (missing)", "/Dest [7 /Fit]", "/Dest [99 0 R /Fit]"] {
            assert_eq!(targets(&annotation(nowhere)), [], "{nowhere}");
        }
        assert_eq!(targets(&annotation("/A << /S /Named /N /PrevPage >>")), []);
    }
}
