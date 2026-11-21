//! Window-level actions and their default key bindings.

use gpui_kit::component::input::{Copy, Cut, Paste, Redo, SelectAll, Undo};
use gpui_kit::{App, KeyBinding, Menu, MenuItem, OsAction, SystemMenuType, actions};

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
        Hide,
        HideOthers,
        ShowAll,
        Quit,
        ResumeSession,
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
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    if cfg!(target_os = "macos") {
        cx.bind_keys([
            KeyBinding::new("cmd-h", Hide, None),
            KeyBinding::new("cmd-alt-h", HideOthers, None),
        ]);
    }
    cx.set_menus(menus());
}

/// The native menu bar. On macOS the first menu is the app menu ("PI"), so the app can be
/// quit, hidden etc. like any other; shortcuts show next to the items automatically.
fn menus() -> Vec<Menu> {
    vec![
        Menu::new("PI").items([
            MenuItem::os_submenu("Services", SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action("Hide PI", Hide),
            MenuItem::action("Hide Others", HideOthers),
            MenuItem::action("Show All", ShowAll),
            MenuItem::separator(),
            MenuItem::action("Quit PI", Quit),
        ]),
        Menu::new("File").items([MenuItem::action("New Chat / Session", NewChat)]),
        Menu::new("Edit").items([
            MenuItem::os_action("Undo", Undo, OsAction::Undo),
            MenuItem::os_action("Redo", Redo, OsAction::Redo),
            MenuItem::separator(),
            MenuItem::os_action("Cut", Cut, OsAction::Cut),
            MenuItem::os_action("Copy", Copy, OsAction::Copy),
            MenuItem::os_action("Paste", Paste, OsAction::Paste),
            MenuItem::os_action("Select All", SelectAll, OsAction::SelectAll),
        ]),
        Menu::new("View").items([
            MenuItem::action("Chat", ShowChatMode),
            MenuItem::action("Code", ShowCodeMode),
            MenuItem::separator(),
            MenuItem::action("Toggle Sidebar", ToggleSidebar),
            MenuItem::action("Toggle Side Panel", ToggleRightPanel),
            MenuItem::action("Changes", ShowDiff),
            MenuItem::action("Files", ShowFiles),
            MenuItem::action("Terminal", ToggleTerminal),
        ]),
        Menu::new("Window").items([MenuItem::action("Focus Message Box", FocusComposer)]),
    ]
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
