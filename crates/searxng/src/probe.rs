//! Whether the SearXNG at an address can be used.

use std::time::Duration;

/// What answers at an address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Probe {
    /// A SearXNG that gives results as JSON.
    Ready,
    /// A SearXNG that refuses to.
    NoJson,
    /// Something else, or a SearXNG that is limiting requests.
    Unexpected(u16),
    Nothing,
}

/// Asks `url` without searching for anything: SearXNG refuses the format first, and then the
/// missing query. Blocks for a few seconds at most.
pub fn probe(url: &str) -> Probe {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(4)))
        .user_agent("pistachio-deepsearch")
        .http_status_as_error(false)
        .build()
        .new_agent();
    let address = format!("{}/search?format=json", url.trim_end_matches('/'));
    let Ok(mut response) = agent.get(&address).call() else {
        return Probe::Nothing;
    };
    let status = response.status().as_u16();
    let body = response
        .body_mut()
        .with_config()
        .limit(64 * 1024)
        .read_to_string()
        .unwrap_or_default();
    let json = serde_json::from_str::<serde_json::Value>(&body).ok();
    match (status, json) {
        (400, Some(answer)) if answer.get("error").is_some() => Probe::Ready,
        (403, _) => Probe::NoJson,
        (status, _) => Probe::Unexpected(status),
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read as _, Write as _};
    use std::net::TcpListener;

    use super::*;

    fn answering(status: &str, body: &str) -> Probe {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let reply = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 4096];
            let n = stream.read(&mut request).unwrap();
            stream.write_all(reply.as_bytes()).unwrap();
            String::from_utf8_lossy(&request[..n]).into_owned()
        });
        let result = probe(&url);
        let request = server.join().unwrap();
        assert!(
            request.starts_with("GET /search?format=json HTTP"),
            "{request}"
        );
        result
    }

    #[test]
    fn tells_what_answers() {
        assert_eq!(
            answering("400 Bad Request", r#"{"error": "No query"}"#),
            Probe::Ready
        );
        assert_eq!(
            answering("403 Forbidden", "<h1>Forbidden</h1>"),
            Probe::NoJson
        );
        assert_eq!(
            answering("200 OK", "<h1>It works</h1>"),
            Probe::Unexpected(200)
        );
        assert_eq!(answering("400 Bad Request", "no"), Probe::Unexpected(400));
        let free = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        assert_eq!(probe(&format!("http://{free}")), Probe::Nothing);
        assert_eq!(probe("not an address"), Probe::Nothing);
    }
}
