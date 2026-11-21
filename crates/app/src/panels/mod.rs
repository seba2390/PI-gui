//! Secondary panels: the right-hand panel (git diff, files) and the bottom terminal.

pub mod right_panel;
pub mod terminal_panel;

pub use right_panel::{RightPanel, RightPanelEvent, RightTab};
pub use terminal_panel::{TerminalPanel, TerminalPanelEvent};
