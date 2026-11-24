//! Search backends. Each turns a query into [`SearchResult`]s; [`search`] picks the one the
//! config names. To add a backend: a variant in [`crate::config::BackendKind`], its settings
//! section in [`crate::config::Config`], and a function here.

use std::time::Duration;

use markup5ever_rcdom::{Handle, NodeData, RcDom};
use serde::Serialize;
use serde_json::Value;

use crate::config::{BackendKind, Config};
use crate::extract::{fragment_text, squash};
use crate::http::{backend_agent, local_agent, read_body};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

const USER_AGENT: &str = "pistachio-deepsearch";
/// How long a search waits for the SearXNG on this computer, which the app starts with the
/// message that searches.
const LOCAL_START: Duration = Duration::from_secs(30);
/// Search responses larger than this are cut off.
const MAX_RESPONSE: usize = 2_000_000;

pub fn search(config: &Config, query: &str, count: usize) -> Result<Vec<SearchResult>, String> {
    let count = count.clamp(1, 20);
    let mut results = match config.backend {
        BackendKind::Duckduckgo => duckduckgo(config, query)?,
        BackendKind::Searxng => searxng(config, query)?,
        BackendKind::Brave => brave(config, query, count)?,
        BackendKind::Tavily => tavily(config, query, count)?,
    };
    let mut seen = std::collections::HashSet::new();
    results.retain(|result| {
        result.url.starts_with("http") && seen.insert(result.url.trim_end_matches('/').to_owned())
    });
    results.truncate(count);
    Ok(results)
}

pub fn backend_name(kind: BackendKind) -> &'static str {
    match kind {
        BackendKind::Duckduckgo => "DuckDuckGo",
        BackendKind::Searxng => "SearXNG",
        BackendKind::Brave => "Brave Search",
        BackendKind::Tavily => "Tavily",
    }
}

fn get_text(
    config: &Config,
    request: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
) -> Result<(u16, String), String> {
    let name = backend_name(config.backend);
    let mut response = request.map_err(|error| format!("{name} could not be reached: {error}"))?;
    let status = response.status().as_u16();
    let (bytes, _) = read_body(response.body_mut(), MAX_RESPONSE)
        .map_err(|error| format!("{name} response could not be read: {error}"))?;
    Ok((status, String::from_utf8_lossy(&bytes).into_owned()))
}

fn parse_json(config: &Config, status: u16, body: &str) -> Result<Value, String> {
    let name = backend_name(config.backend);
    if status == 401 || status == 403 {
        return Err(format!(
            "{name} refused the request (HTTP {status}). Check the API key{}.",
            if config.backend == BackendKind::Searxng {
                " or enable the json format in the instance's settings.yml (search.formats)"
            } else {
                ""
            }
        ));
    }
    if status == 429 {
        return Err(format!(
            "{name} rate limit reached (HTTP 429). Wait a bit before searching again."
        ));
    }
    if !(200..300).contains(&status) {
        return Err(format!("{name} answered HTTP {status}: {}", snippet(body)));
    }
    serde_json::from_str(body)
        .map_err(|error| format!("{name} sent a response that is not JSON ({error})"))
}

fn snippet(body: &str) -> String {
    squash(&fragment_text(body)).chars().take(200).collect()
}

fn str_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(fragment_text)
        .unwrap_or_default()
}

fn results_from(array: Option<&Value>, title: &str, url: &str, snippet: &str) -> Vec<SearchResult> {
    array
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|item| SearchResult {
            title: str_field(item, title),
            url: item
                .get(url)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            snippet: str_field(item, snippet),
        })
        .collect()
}

fn searxng(config: &Config, query: &str) -> Result<Vec<SearchResult>, String> {
    let settings = &config.searxng;
    let (agent, url) = if settings.local {
        let dir = crate::config::default_dir().ok_or("The settings folder is unknown.")?;
        (
            local_agent(config.search.timeout_secs, USER_AGENT),
            searxng::wait_for_url(&searxng::root_in(&dir), LOCAL_START)?,
        )
    } else {
        (
            backend_agent(config.search.timeout_secs, USER_AGENT),
            settings.url.clone(),
        )
    };
    let mut request = agent
        .get(format!("{}/search", url.trim_end_matches('/')))
        .query("q", query)
        .query("format", "json");
    if !settings.engines.trim().is_empty() {
        request = request.query("engines", settings.engines.trim());
    }
    if !settings.language.trim().is_empty() {
        request = request.query("language", settings.language.trim());
    }
    let (status, body) = get_text(config, request.call())?;
    let json = parse_json(config, status, &body)?;
    Ok(results_from(json.get("results"), "title", "url", "content"))
}

fn brave(config: &Config, query: &str, count: usize) -> Result<Vec<SearchResult>, String> {
    let key = config.brave.resolve().ok_or_else(|| {
        format!(
            "Brave Search needs an API key: set brave.api_key in config.json or the {} \
             environment variable.",
            config.brave.api_key_env
        )
    })?;
    let agent = backend_agent(config.search.timeout_secs, USER_AGENT);
    let request = agent
        .get("https://api.search.brave.com/res/v1/web/search")
        .query("q", query)
        .query("count", count.to_string())
        .header("Accept", "application/json")
        .header("X-Subscription-Token", key)
        .call();
    let (status, body) = get_text(config, request)?;
    let json = parse_json(config, status, &body)?;
    Ok(results_from(
        json.get("web").and_then(|web| web.get("results")),
        "title",
        "url",
        "description",
    ))
}

fn tavily(config: &Config, query: &str, count: usize) -> Result<Vec<SearchResult>, String> {
    let key = config.tavily.key().resolve().ok_or_else(|| {
        format!(
            "Tavily needs an API key: set tavily.api_key in config.json or the {} environment \
             variable.",
            config.tavily.api_key_env
        )
    })?;
    let agent = backend_agent(config.search.timeout_secs, USER_AGENT);
    let body = serde_json::json!({
        "query": query,
        "max_results": count,
        "search_depth": config.tavily.search_depth,
    });
    let request = agent
        .post("https://api.tavily.com/search")
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {key}"))
        .send(body.to_string());
    let (status, body) = get_text(config, request)?;
    let json = parse_json(config, status, &body)?;
    Ok(results_from(json.get("results"), "title", "url", "content"))
}

/// `DuckDuckGo`'s no-JavaScript "lite" page (no key; rate limited when used heavily).
fn duckduckgo(config: &Config, query: &str) -> Result<Vec<SearchResult>, String> {
    let settings = &config.duckduckgo;
    let safe = match settings.safe_search.as_str() {
        "strict" => "1",
        "off" => "-2",
        _ => "-1",
    };
    let agent = backend_agent(config.search.timeout_secs, &config.fetch.user_agent);
    let request = agent
        .post("https://lite.duckduckgo.com/lite/")
        .header("Referer", "https://lite.duckduckgo.com/")
        .send_form([("q", query), ("kl", settings.region.as_str()), ("kp", safe)]);
    let (status, body) = get_text(config, request)?;
    if status == 202 || body.contains("anomaly-modal") || body.contains("challenge-form") {
        return Err(
            "DuckDuckGo is rate limiting these searches (bot check). Wait a minute, or switch \
             `backend` in the deep search config.json to searxng, brave or tavily."
                .into(),
        );
    }
    if !(200..300).contains(&status) {
        return Err(format!("DuckDuckGo answered HTTP {status}"));
    }
    Ok(parse_duckduckgo_lite(&body))
}

pub fn parse_duckduckgo_lite(html: &str) -> Vec<SearchResult> {
    use html5ever::tendril::TendrilSink as _;
    let dom =
        html5ever::parse_document(RcDom::default(), html5ever::ParseOpts::default()).one(html);
    let mut results: Vec<SearchResult> = Vec::new();
    walk_lite(&dom.document, &mut results, 0);
    // Ads were kept (with no URL) only so their snippets do not attach to real results.
    results.retain(|result| !result.url.is_empty());
    results
}

fn walk_lite(node: &Handle, results: &mut Vec<SearchResult>, depth: usize) {
    if depth > 200 {
        return;
    }
    if let NodeData::Element { name, attrs, .. } = &node.data {
        let attrs = attrs.borrow();
        let get = |key: &str| {
            attrs
                .iter()
                .find(|attribute| &*attribute.name.local == key)
                .map(|attribute| attribute.value.to_string())
        };
        let class = get("class").unwrap_or_default();
        let has_class = |wanted: &str| class.split_whitespace().any(|word| word == wanted);
        if &*name.local == "a" && has_class("result-link") {
            let href = get("href").unwrap_or_default();
            results.push(SearchResult {
                title: squash(&node_text(node)),
                url: result_url(&href).unwrap_or_default(),
                snippet: String::new(),
            });
            return;
        }
        if has_class("result-snippet") {
            if let Some(last) = results.last_mut()
                && last.snippet.is_empty()
            {
                last.snippet = squash(&node_text(node));
            }
            return;
        }
    }
    for child in node.children.borrow().iter() {
        walk_lite(child, results, depth + 1);
    }
}

/// The target of a result link: `DuckDuckGo`'s redirect (`/l/?uddg=…`) unwrapped, ads dropped.
fn result_url(href: &str) -> Option<String> {
    let absolute = if href.starts_with("//") {
        format!("https:{href}")
    } else {
        href.to_owned()
    };
    let parsed = url::Url::parse(&absolute).ok()?;
    let host = parsed.host_str().unwrap_or_default();
    if host.ends_with("duckduckgo.com") {
        if parsed.path().starts_with("/y.js") {
            return None;
        }
        return parsed
            .query_pairs()
            .find(|(key, _)| key == "uddg")
            .map(|(_, value)| value.into_owned());
    }
    Some(absolute)
}

fn node_text(node: &Handle) -> String {
    let mut text = String::new();
    let mut stack = vec![node.clone()];
    while let Some(node) = stack.pop() {
        if let NodeData::Text { contents } = &node.data {
            text.push_str(&contents.borrow());
        }
        stack.extend(node.children.borrow().iter().rev().cloned());
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn parses_duckduckgo_lite_results() {
        let html = r#"<table>
          <tr><td>1.</td><td><a rel="nofollow" href="https://docs.rs/html5ever/" class='result-link'>html5ever - <b>Rust</b></a></td></tr>
          <tr><td></td><td class='result-snippet'>API documentation for the <b>Rust</b> crate.</td></tr>
          <tr><td>2.</td><td><a href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fa%3Fb%3D1&rut=x" class="result-link">Example</a></td></tr>
          <tr><td>3.</td><td><a href="https://duckduckgo.com/y.js?ad=1" class="result-link">Ad</a></td></tr>
          <tr><td></td><td class='result-snippet'>Ad text</td></tr>
        </table>"#;
        assert_eq!(
            parse_duckduckgo_lite(html),
            vec![
                SearchResult {
                    title: "html5ever - Rust".into(),
                    url: "https://docs.rs/html5ever/".into(),
                    snippet: "API documentation for the Rust crate.".into(),
                },
                SearchResult {
                    title: "Example".into(),
                    url: "https://example.com/a?b=1".into(),
                    snippet: String::new(),
                },
            ]
        );
    }
}
