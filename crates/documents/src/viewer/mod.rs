//! Showing PDF files. Pages are drawn by a helper process (the app's own executable, see
//! [`crate::cli`]) that stays while a file is shown, so that a file that breaks its renderer
//! never takes the app down ([`Viewer`]).

use serde::{Deserialize, Serialize};

mod client;
mod dark;
pub mod layout;
mod links;
mod render;
pub(crate) mod serve;
#[cfg(test)]
mod testing;

pub use client::Viewer;

/// A page's size, in points.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Size {
    pub width: f32,
    pub height: f32,
}

/// A page to draw.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PageRequest {
    pub page: usize,
    /// Pixels per point.
    pub scale: f32,
    /// With light text on a dark page.
    pub dark: bool,
}

/// A page as it was drawn.
#[derive(Debug, Clone, PartialEq)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub links: Vec<Link>,
    /// Opaque, as blue, green, red and alpha.
    pub pixels: Vec<u8>,
}

/// Where a click leads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Link {
    /// In points from the page's top left corner.
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub target: Target,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Target {
    /// Only `http`, `https` and `mailto` ones.
    Uri(String),
    /// A place in the file, in points from the top of the page.
    Page { page: usize, y: f32 },
}
