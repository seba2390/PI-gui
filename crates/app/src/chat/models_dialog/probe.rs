//! Asking model servers: which run here, what one offers, whether a model or a key works.
//! Everything blocks; call it on the background executor.

use std::time::{Duration, Instant};

use pi_config::providers::BuiltIn;
use pi_config::server::{self, Kind, LocalServer, Offer};

use crate::pi_place::{NoReply, PiPlace, Reply};

/// How long a server on this computer may take to answer.
const LOCAL: Duration = Duration::from_millis(1500);
const LIST: Duration = Duration::from_secs(10);
/// A server may have to load the model first.
const TRIAL: Duration = Duration::from_secs(90);

/// What came of connecting to a server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Connection {
    Online {
        base_url: String,
        offer: Offer,
    },
    /// It wants a key, or another one.
    NeedsKey {
        base_url: String,
    },
    /// It answers, but not with a list of models.
    NoList {
        base_url: String,
        message: String,
    },
    Offline(String),
    /// The address is offline, but models with an address of their own are there.
    Elsewhere,
}

/// Finds the API of the server at one of `base_urls` (the likeliest first) and what it offers.
pub fn connect(place: &PiPlace, base_urls: &[String], key: Option<&str>) -> Connection {
    let mut offline = None;
    for base_url in base_urls {
        match connect_to(place, base_url, key) {
            Connection::Offline(message) => {
                offline.get_or_insert(message);
            }
            connection => return connection,
        }
    }
    Connection::Offline(offline.unwrap_or_else(|| "Enter the server's address.".to_owned()))
}

fn connect_to(place: &PiPlace, base_url: &str, key: Option<&str>) -> Connection {
    let mut needs_key = None;
    let mut problem: Option<(String, String)> = None;
    for attempt in server::attempts(base_url) {
        let headers = server::headers(attempt.api, key);
        let reply = match place.request(&attempt.url, &headers, None, LIST) {
            Ok(reply) => reply,
            Err(no_reply) => return Connection::Offline(describe(&no_reply, base_url)),
        };
        match reply.status {
            200..=299 => match server::parse_offer(&reply.body) {
                Ok(mut offer) => {
                    let base_url = match offer.api {
                        api if api == attempt.api => attempt.base_url,
                        // The same URL lists both, but pi adds the version itself.
                        "anthropic-messages" => attempt.base_url.trim_end_matches("/v1").to_owned(),
                        _ => attempt.base_url,
                    };
                    for question in server::questions(&base_url, &offer) {
                        let headers = server::headers(offer.api, key);
                        if let Ok(answer) =
                            place.request(&question.url, &headers, question.body.as_deref(), LIST)
                            && answer.status == 200
                        {
                            offer.add_answer(&question, &answer.body);
                        }
                    }
                    return Connection::Online { base_url, offer };
                }
                Err(error) => {
                    problem.get_or_insert((attempt.base_url, error.to_string()));
                }
            },
            401 | 403 => {
                needs_key.get_or_insert(attempt.base_url);
            }
            _ => {
                problem.get_or_insert((attempt.base_url, refusal(&reply)));
            }
        }
    }
    match (needs_key, problem) {
        (Some(base_url), _) => Connection::NeedsKey { base_url },
        (None, Some((base_url, message))) => Connection::NoList { base_url, message },
        (None, None) => Connection::Offline(describe(&NoReply::Refused, base_url)),
    }
}

fn refusal(reply: &Reply) -> String {
    match server::error_message(&reply.body) {
        Some(message) => format!("It answered {}: {message}", reply.status),
        None => format!("It answered {}.", reply.status),
    }
}

/// What to tell when nothing came back.
fn describe(no_reply: &NoReply, base_url: &str) -> String {
    let address = server::short_address(base_url);
    match no_reply {
        NoReply::Refused => format!("Nothing answers at {address}. Is the server running?"),
        NoReply::UnknownHost => {
            format!("There is no computer called {}.", server::host_of(base_url))
        }
        NoReply::Timeout => format!(
            "{address} did not answer in time. Is the server running, and can this computer reach it?"
        ),
        NoReply::Other(message) => format!("Could not connect to {address}: {message}"),
    }
}

/// How many models the server that usually listens at this address has, and which program it
/// is. `None` when it is not running.
pub fn find(place: &PiPlace, local: &LocalServer) -> Option<(Kind, usize)> {
    let url = format!("{}/models", local.address);
    let reply = place.request(&url, &[], None, LOCAL).ok()?;
    let offer = server::parse_offer(&reply.body).ok()?;
    let kind = match offer.kind {
        // Any program may listen there; only Ollama and LM Studio have a port of their own.
        Kind::Other if matches!(local.kind, Kind::Ollama | Kind::LmStudio) => local.kind,
        kind => kind,
    };
    let models = offer.models.iter().filter(|model| !model.not_chat).count();
    Some((kind, models))
}

/// Whether anything answers at `base_url`, whatever it says.
pub fn answers(place: &PiPlace, base_url: &str) -> bool {
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    place
        .request(&url, &[], None, Duration::from_secs(4))
        .is_ok()
}

/// Has `model` say something. How long it took, or what went wrong.
pub fn try_model(
    place: &PiPlace,
    api: &str,
    base_url: &str,
    key: Option<&str>,
    model: &str,
) -> Result<Duration, String> {
    let (url, body) = server::trial(api, base_url, model)
        .ok_or_else(|| format!("Models of “{api}” can't be tested here."))?;
    let headers = server::headers(api, key);
    let start = Instant::now();
    let reply = place
        .request(&url, &headers, Some(&body), TRIAL)
        .map_err(|no_reply| describe(&no_reply, base_url))?;
    match reply.status {
        200..=299 => Ok(start.elapsed()),
        401 | 403 => Err("The server did not accept the API key.".to_owned()),
        _ => Err(refusal(&reply)),
    }
}

/// Whether a provider takes a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyCheck {
    Accepted,
    Refused(String),
    /// It can't be told: there is no connection, or the key is looked up by pi.
    Unknown(String),
}

pub fn check_key(place: &PiPlace, provider: &BuiltIn, key: &str) -> KeyCheck {
    let (Some(check), Some(key)) = (provider.check, pi_config::literal_key(key)) else {
        return KeyCheck::Accepted;
    };
    let headers = server::headers(check.api, Some(key));
    match place.request(check.url, &headers, None, LIST) {
        Ok(reply) if (200..300).contains(&reply.status) => KeyCheck::Accepted,
        Ok(reply) if matches!(reply.status, 400 | 401 | 403) => {
            KeyCheck::Refused(match server::error_message(&reply.body) {
                Some(message) => format!("{} did not accept the key: {message}", provider.label),
                None => format!("{} did not accept the key.", provider.label),
            })
        }
        Ok(reply) => KeyCheck::Unknown(format!(
            "{} could not check the key: it answered {}.",
            provider.label, reply.status
        )),
        Err(no_reply) => KeyCheck::Unknown(describe(&no_reply, check.url)),
    }
}

#[cfg(test)]
pub(super) mod tests {
    use std::io::{BufRead as _, BufReader, Read as _, Write as _};
    use std::net::TcpListener;

    use pretty_assertions::assert_eq;

    use super::*;

    /// A server that answers each request with what `answer` makes of its method, path, headers
    /// and body, as long as the test runs. Returns its address.
    pub fn serve(
        answer: impl Fn(&str, &str, &str, &str) -> (u16, String) + Send + 'static,
    ) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut first = String::new();
                if reader.read_line(&mut first).unwrap_or(0) == 0 {
                    continue;
                }
                let mut headers = String::new();
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                        break;
                    }
                    let line = line.to_lowercase();
                    if let Some(value) = line.strip_prefix("content-length:") {
                        length = value.trim().parse().unwrap();
                    }
                    headers.push_str(&line);
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                let mut words = first.split_whitespace();
                let (method, path) = (words.next().unwrap(), words.next().unwrap());
                let (status, reply) =
                    answer(method, path, &headers, &String::from_utf8_lossy(&body));
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                    reply.len()
                );
            }
        });
        address
    }

    /// An Ollama with one model for conversations and one that is not.
    pub fn ollama(method: &str, path: &str, _: &str, body: &str) -> (u16, String) {
        match (method, path) {
            ("GET", "/v1/models") => (
                200,
                r#"{"data":[{"id":"qwen3:8b","owned_by":"library"},{"id":"nomic-embed-text","owned_by":"library"}]}"#.into(),
            ),
            ("POST", "/api/show") if body.contains("qwen3") => (
                200,
                r#"{"capabilities":["completion","thinking"],"model_info":{"qwen3.context_length":40960}}"#.into(),
            ),
            ("POST", "/v1/chat/completions") if body.contains("qwen3") => (200, "{}".into()),
            _ => (404, r#"{"error":"not found"}"#.into()),
        }
    }

    fn here() -> PiPlace {
        PiPlace::Local {
            agent_dir: std::env::temp_dir(),
        }
    }

    fn connect_to_address(address: &str, key: Option<&str>) -> Connection {
        connect(&here(), &server::base_urls(address).unwrap(), key)
    }

    #[test]
    fn connects_to_a_server_by_its_address() {
        let address = serve(ollama);
        let Connection::Online { base_url, offer } = connect_to_address(&address, None) else {
            panic!("not online");
        };
        assert_eq!(base_url, format!("http://{address}/v1"));
        assert_eq!(offer.kind, Kind::Ollama);
        assert_eq!(offer.models[1].context_window, Some(40_960));
        assert_eq!(offer.models[1].reasoning, Some(true));
        assert!(offer.models[0].not_chat);

        let trial = |model| try_model(&here(), offer.api, &base_url, None, model);
        assert!(trial("qwen3:8b").is_ok());
        assert_eq!(trial("x").unwrap_err(), "It answered 404: not found");
    }

    #[test]
    fn asks_for_a_key() {
        let address = serve(|_, path, headers, _| match path {
            "/v1/models" if headers.contains("authorization: bearer good") => {
                (200, r#"{"data":[{"id":"m"}]}"#.into())
            }
            _ => (401, r#"{"error":{"message":"no key"}}"#.into()),
        });
        let needs_key = Connection::NeedsKey {
            base_url: format!("http://{address}/v1"),
        };
        assert_eq!(connect_to_address(&address, None), needs_key);
        assert_eq!(connect_to_address(&address, Some("bad")), needs_key);
        assert!(matches!(
            connect_to_address(&address, Some("good")),
            Connection::Online { .. }
        ));
    }

    #[test]
    fn finds_the_anthropic_api() {
        let address = serve(|_, path, headers, _| {
            match path {
            "/v1/models" if headers.contains("x-api-key: k") => (
                200,
                r#"{"data":[{"type":"model","id":"claude","display_name":"Claude"}],"has_more":false}"#.into(),
            ),
            _ => (401, "{}".into()),
        }
        });
        let Connection::Online { base_url, offer } = connect_to_address(&address, Some("k")) else {
            panic!("not online");
        };
        assert_eq!(offer.api, "anthropic-messages");
        assert_eq!(base_url, format!("http://{address}"));
    }

    #[test]
    fn says_why_there_is_no_list() {
        let address = serve(|_, _, _, _| (200, "<h1>It works</h1>".into()));
        assert!(matches!(
            connect_to_address(&address, None),
            Connection::NoList { message, .. } if message.contains("It works")
        ));
        let free = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .to_string();
        assert_eq!(
            connect_to_address(&free, None),
            Connection::Offline(format!("Nothing answers at {free}. Is the server running?"))
        );
        assert!(!answers(&here(), &format!("http://{free}/v1")));
        assert!(answers(&here(), &format!("http://{address}/v1")));
    }

    #[test]
    fn checks_keys() {
        let address = serve(|_, _, headers, _| {
            if headers.contains("bearer good") {
                (200, "{}".into())
            } else {
                (401, r#"{"error":{"message":"Incorrect API key"}}"#.into())
            }
        });
        let url: &'static str = format!("http://{address}/v1/models").leak();
        let mut provider = *pi_config::providers::built_in("openai").unwrap();
        provider.check = Some(pi_config::providers::KeyCheck {
            url,
            api: "openai-completions",
        });
        let check = |key| check_key(&here(), &provider, key);
        assert_eq!(check("good"), KeyCheck::Accepted);
        assert_eq!(
            check("bad"),
            KeyCheck::Refused("OpenAI did not accept the key: Incorrect API key".into())
        );
        // pi looks these up itself.
        assert_eq!(check("$OPENAI_API_KEY"), KeyCheck::Accepted);
        assert_eq!(check("!pass openai"), KeyCheck::Accepted);
    }
}
