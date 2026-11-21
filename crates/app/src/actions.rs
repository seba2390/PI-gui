//! Window-level actions and their default key bindings.

use gpui_kit::{App, KeyBinding, actions};

actions!(
    pi_gui,
    [
        NewChat,
        ToggleSidebar,
        ToggleRightPanel,
        ToggleTerminal,
        ShowDiff,
        ShowFiles,
        FocusComposer,
        ShowChatMode,
        ShowCodeMode,
        Quit,
    ]
);

/// Shortcuts shown in the Chat | Code switch's tooltips.
pub const CHAT_MODE_KEYS: &str = if cfg!(target_os = "macos") {
    "⌘1"
} else {
    "Ctrl+1"
};
pub const CODE_MODE_KEYS: &str = if cfg!(target_os = "macos") {
    "⌘2"
} else {
    "Ctrl+2"
};

/// Key context of the main window.
pub const WINDOW_CONTEXT: &str = "PiWindow";

/// `primary!("n")` → `"cmd-n"` on macOS, `"ctrl-n"` elsewhere (as `&'static str`).
macro_rules! primary {
    ($keys:literal) => {
        if cfg!(target_os = "macos") {
            concat!("cmd-", $keys)
        } else {
            concat!("ctrl-", $keys)
        }
    };
}

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new(primary!("n"), NewChat, Some(WINDOW_CONTEXT)),
        KeyBinding::new(primary!("b"), ToggleSidebar, Some(WINDOW_CONTEXT)),
        KeyBinding::new(primary!("alt-b"), ToggleRightPanel, Some(WINDOW_CONTEXT)),
        KeyBinding::new("ctrl-`", ToggleTerminal, Some(WINDOW_CONTEXT)),
        KeyBinding::new(primary!("shift-d"), ShowDiff, Some(WINDOW_CONTEXT)),
        KeyBinding::new(primary!("shift-e"), ShowFiles, Some(WINDOW_CONTEXT)),
        KeyBinding::new(primary!("l"), FocusComposer, Some(WINDOW_CONTEXT)),
        KeyBinding::new(primary!("1"), ShowChatMode, Some(WINDOW_CONTEXT)),
        KeyBinding::new(primary!("2"), ShowCodeMode, Some(WINDOW_CONTEXT)),
        KeyBinding::new(primary!("q"), Quit, None),
    ]);
    cx.on_action(|_: &Quit, cx| cx.quit());
}

#[cfg(test)]
mod tests {
    use gpui_kit::Keystroke;

    /// Every binding must parse; an invalid one panics at startup.
    #[test]
    fn bindings_parse() {
        let primary = [
            primary!("1"),
            primary!("2"),
            primary!("n"),
            primary!("b"),
            primary!("alt-b"),
            primary!("shift-d"),
            primary!("l"),
        ];
        for keys in primary {
            assert!(Keystroke::parse(keys).is_ok(), "{keys}");
        }
        for keys in [
            "ctrl-`",
            "alt-enter",
            "alt-up",
            "shift-tab",
            "escape",
            "ctrl-t",
            "ctrl-o",
        ] {
            assert!(Keystroke::parse(keys).is_ok(), "{keys}");
        }
    }
}
