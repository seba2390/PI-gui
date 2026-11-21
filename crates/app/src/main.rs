//! PI-gui: a lean desktop GUI for the pi coding agent.
//!
//! The GUI is a pure view over `pi --mode rpc`: every agent behaviour comes from pi
//! itself; this binary only renders pi's event stream and forwards user input.

mod actions;
mod assets;
mod chat;
mod config;
mod panels;
mod shell_env;
mod sidebar;
mod theme;
mod ui;
mod window;

use gpui_kit::*;

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn,pi_gui=info"))
        .init();

    // Probing the login shell takes ~100–200 ms; overlap it with GPU/platform startup.
    let shell_env = std::thread::spawn(shell_env::login_shell_env);

    application().with_assets(assets::AppAssets).run(move |cx| {
        let child_env = shell_env.join().unwrap_or_default();
        let config = config::AppConfig::detect(child_env);
        init(cx);
        theme::init(cx);
        actions::init(cx);
        cx.set_global(config);
        window::open_main_window(cx);
        cx.activate(true);
    });
}
