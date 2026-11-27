//! Really downloads, starts and asks a model. Ignored: it downloads gigabytes.
//!
//! `LOCAL_MODELS_LIVE=<model>/<build>/<engine> cargo test -p local-models --test live -- --ignored --nocapture`
//! (for example `lfm3-1.7b/gguf-q4_k_m/llama-cpp`). `LOCAL_MODELS_LIVE_DIR` keeps the downloads.

use std::sync::atomic::AtomicBool;

use local_models::recommend::choices;
use local_models::{Server, Setup, catalog, hardware, install, setups};

#[test]
#[ignore = "downloads a model"]
fn installs_starts_and_answers() {
    let key = std::env::var("LOCAL_MODELS_LIVE")
        .unwrap_or_else(|_| "lfm3-1.7b/gguf-q4_k_m/llama-cpp".into());
    let dir = std::env::var_os("LOCAL_MODELS_LIVE_DIR").map_or_else(
        || std::env::temp_dir().join("local-models-live"),
        Into::into,
    );
    let hardware = hardware::scan(&dir, None);
    let choice = choices(&hardware, catalog())
        .into_iter()
        .find(|choice| choice.key() == key)
        .unwrap_or_else(|| panic!("{key} does not run on this computer"));
    let cancel = AtomicBool::new(false);
    let started = std::time::Instant::now();
    install::install(&dir, &choice, None, &cancel, &|progress| {
        eprint!(
            "\r{} {:.0}%   ",
            progress.stage.label(),
            progress.fraction() * 100.
        );
    })
    .unwrap();
    eprintln!("\ninstalled in {:?}", started.elapsed());
    let setup = Setup {
        model: choice.model.id.clone(),
        build: choice.build.id.clone(),
        engine: choice.engine.id.clone(),
        port: setups::free_port(&[]).unwrap(),
        context: 8192,
    };
    let started = std::time::Instant::now();
    let server = Server::start(&dir, catalog(), &setup, None).unwrap();
    eprintln!("started in {:?} on {}", started.elapsed(), setup.base_url());
    let agent = ureq::Agent::new_with_defaults();
    let models: String = agent
        .get(format!("{}/models", setup.base_url()))
        .call()
        .unwrap()
        .body_mut()
        .read_to_string()
        .unwrap();
    eprintln!("models: {models}");
    let body = serde_json::json!({
        "model": serde_json::from_str::<serde_json::Value>(&models).unwrap()["data"][0]["id"],
        "messages": [{"role": "user", "content": "Say hello in three words."}],
        "max_tokens": 40,
    });
    let started = std::time::Instant::now();
    let answer: String = agent
        .post(format!("{}/chat/completions", setup.base_url()))
        .header("content-type", "application/json")
        .send(body.to_string())
        .unwrap()
        .body_mut()
        .read_to_string()
        .unwrap();
    eprintln!("answered in {:?}: {answer}", started.elapsed());
    assert!(answer.contains("choices"));
    drop(server);
}
