//! The proxy for a limited network ([`Network::Off`] and [`Network::Custom`]): the app runs it,
//! and sandboxed commands can reach nothing else. It reads where a connection goes (HTTPS
//! `CONNECT host:port`, or a plain HTTP request's address) and checks the site against the saved
//! list (read again for each connection, so changes apply at once). A site not on it is asked
//! about in the conversation the command belongs to ([`ask`]): the proxy address commands get
//! names their session (`http://pi-<pid>:x@127.0.0.1:port`), whose gate socket asks the user like
//! a `git push` does. Then it connects, or answers 403 saying why.
//!
//! Commands find it through `HTTPS_PROXY` and the like, which curl, git, npm, pip, cargo and
//! most other tools follow (Node with `NODE_USE_ENV_PROXY`). Tools that ignore them get no
//! network at all.
//!
//! On macOS commands reach it on a loopback port. With bubblewrap a command has a network of its
//! own, so the proxy also listens on a Unix socket in the sandbox's folder, mounted into the
//! sandbox, where [`bridge`] passes the command's connections on to it.
//!
//! [`Network::Off`]: crate::Network::Off
//! [`Network::Custom`]: crate::Network::Custom

use std::collections::HashMap;
use std::io::{self, BufRead as _, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream, ToSocketAddrs as _};
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::settings::Settings;

/// Where the running proxy listens, in the sandbox's folder.
const ENDPOINT_FILE: &str = "proxy.json";
const SOCKET_FILE: &str = "proxy.sock";
/// The longest request head read before it is refused.
const HEAD_LIMIT: usize = 32 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// Where the proxy listens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Endpoint {
    /// On the loopback address.
    pub port: u16,
    /// A Unix socket (not on Windows).
    pub socket: Option<PathBuf>,
}

impl Endpoint {
    /// The proxy address commands are given.
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// The environment that sends commands' requests through the proxy, naming `session` (see
    /// [`session_of`]) so that sites are asked about in its conversation.
    pub fn env_for(&self, session: Option<&str>) -> Vec<(&'static str, String)> {
        let url = match session {
            Some(session) => format!("http://{session}:x@127.0.0.1:{}", self.port),
            None => self.url(),
        };
        let mut env: Vec<(&'static str, String)> = [
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
        ]
        .into_iter()
        .map(|name| (name, url.clone()))
        .collect();
        env.extend([
            ("NO_PROXY", String::new()),
            ("no_proxy", String::new()),
            ("NODE_USE_ENV_PROXY", "1".to_owned()),
        ]);
        env
    }
}

/// The session a gate socket (`<dir>/<pi's pid>.sock`) belongs to, as the proxy address names it:
/// `pi-<pid>`.
pub fn session_of(socket: &Path) -> Option<String> {
    let pid: u32 = socket.file_stem()?.to_str()?.parse().ok()?;
    Some(format!("pi-{pid}"))
}

/// The running proxy's endpoint, as the app saved it in `dir`.
pub fn endpoint(dir: &Path) -> Option<Endpoint> {
    let text = std::fs::read_to_string(dir.join(ENDPOINT_FILE)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Starts the proxy for the settings in `dir` (on background threads, for as long as this
/// process runs) and saves where it listens.
pub fn start(dir: &Path) -> io::Result<Endpoint> {
    std::fs::create_dir_all(dir)?;
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    let settings_dir = dir.to_path_buf();
    std::thread::Builder::new()
        .name("sandbox-proxy".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                let dir = settings_dir.clone();
                std::thread::spawn(move || serve(stream, &dir));
            }
        })?;
    let socket = start_socket(dir);
    let endpoint = Endpoint { port, socket };
    let text = serde_json::to_string(&endpoint).map_err(io::Error::other)?;
    let temp = dir.join(format!("{ENDPOINT_FILE}.tmp"));
    std::fs::write(&temp, text)?;
    std::fs::rename(temp, dir.join(ENDPOINT_FILE))?;
    Ok(endpoint)
}

#[cfg(unix)]
fn start_socket(dir: &Path) -> Option<PathBuf> {
    use std::os::unix::net::UnixListener;
    let path = dir.join(SOCKET_FILE);
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).ok()?;
    let settings_dir = dir.to_path_buf();
    std::thread::Builder::new()
        .name("sandbox-proxy-socket".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                let dir = settings_dir.clone();
                std::thread::spawn(move || serve(stream, &dir));
            }
        })
        .ok()?;
    Some(path)
}

#[cfg(not(unix))]
fn start_socket(_dir: &Path) -> Option<PathBuf> {
    None
}

/// A connection that can be read and written from two threads and closed for writing.
pub trait Duplex: Read + Write + Send + Sized + 'static {
    fn try_clone(&self) -> io::Result<Self>;
    fn close_write(&self);
}

impl Duplex for TcpStream {
    fn try_clone(&self) -> io::Result<Self> {
        TcpStream::try_clone(self)
    }
    fn close_write(&self) {
        let _ = self.shutdown(Shutdown::Write);
    }
}

#[cfg(unix)]
impl Duplex for std::os::unix::net::UnixStream {
    fn try_clone(&self) -> io::Result<Self> {
        std::os::unix::net::UnixStream::try_clone(self)
    }
    fn close_write(&self) {
        let _ = self.shutdown(Shutdown::Write);
    }
}

/// Where a request goes and how to pass it on.
#[derive(Debug, PartialEq, Eq)]
struct Request {
    host: String,
    port: u16,
    /// `CONNECT`: a tunnel, answered with 200 before anything is passed on.
    tunnel: bool,
    /// For plain HTTP: the request head to send, with the address turned into a path.
    head: Vec<u8>,
    /// The session the proxy address named (`Proxy-Authorization`'s user).
    session: Option<String>,
}

/// Reads where a request head goes: `CONNECT host:port`, or `METHOD http://host[:port]/path`.
fn parse_head(head: &[u8]) -> Result<Request, &'static str> {
    let text = std::str::from_utf8(head).map_err(|_| "not a request")?;
    let mut lines = text.split("\r\n");
    let first = lines.next().ok_or("not a request")?;
    let mut parts = first.split(' ');
    let (Some(method), Some(target), Some(version)) = (parts.next(), parts.next(), parts.next())
    else {
        return Err("not a request");
    };
    let session = text.split("\r\n").skip(1).find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if !name.trim().eq_ignore_ascii_case("proxy-authorization") {
            return None;
        }
        let encoded = value.trim().strip_prefix("Basic ")?;
        let decoded = String::from_utf8(base64_decode(encoded.trim())?).ok()?;
        let user = decoded.split(':').next()?.to_owned();
        user.starts_with("pi-").then_some(user)
    });
    if method.eq_ignore_ascii_case("CONNECT") {
        let (host, port) = split_host(target, 443).ok_or("bad address")?;
        return Ok(Request {
            host,
            port,
            tunnel: true,
            head: Vec::new(),
            session,
        });
    }
    let rest = target
        .strip_prefix("http://")
        .ok_or("only http:// addresses and HTTPS tunnels")?;
    let (authority, path) = match rest.find('/') {
        Some(at) => (&rest[..at], &rest[at..]),
        None => (rest, "/"),
    };
    let (host, port) = split_host(authority, 80).ok_or("bad address")?;
    let mut out = format!("{method} {path} {version}\r\n");
    for line in lines {
        let name = line.split(':').next().unwrap_or("").to_ascii_lowercase();
        if name.starts_with("proxy-") {
            continue;
        }
        out.push_str(line);
        out.push_str("\r\n");
    }
    // `split("\r\n")` ended with the two empty lines that end the head; one is back.
    let head = out.trim_end_matches("\r\n").to_owned() + "\r\n\r\n";
    Ok(Request {
        host,
        port,
        tunnel: false,
        head: head.into_bytes(),
        session,
    })
}

/// Standard base64 (what `Proxy-Authorization: Basic` holds), or `None`.
fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let value = |byte: u8| -> Option<u32> {
        Some(u32::from(match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        }))
    };
    let bytes = text.trim_end_matches('=').as_bytes();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        let mut buffer = 0u32;
        for (index, &byte) in chunk.iter().enumerate() {
            buffer |= value(byte)? << (18 - 6 * index);
        }
        let take = match chunk.len() {
            4 => 3,
            3 => 2,
            2 => 1,
            _ => return None,
        };
        out.extend_from_slice(&buffer.to_be_bytes()[1..=take]);
    }
    Some(out)
}

/// `host:port` (IPv6 in brackets) with `default` for a missing port.
fn split_host(authority: &str, default: u16) -> Option<(String, u16)> {
    let authority = authority.rsplit('@').next()?;
    let (host, port) = if let Some(rest) = authority.strip_prefix('[') {
        let (host, after) = rest.split_once(']')?;
        let port = match after.strip_prefix(':') {
            Some(port) => port.parse().ok()?,
            None => default,
        };
        (host, port)
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) => (host, port.parse().ok()?),
            None => (authority, default),
        }
    };
    (!host.is_empty()).then(|| (host.to_ascii_lowercase(), port))
}

/// Handles one connection from a command.
fn serve<S: Duplex>(mut client: S, dir: &Path) {
    let Ok((head, rest)) = read_head(&mut client) else {
        return;
    };
    let request = match parse_head(&head) {
        Ok(request) => request,
        Err(why) => {
            let _ = refuse(&mut client, 400, why);
            return;
        }
    };
    let settings = Settings::load(dir);
    let listed = settings.network == crate::Network::On
        || (settings.network == crate::Network::Custom
            && crate::sites::allowed(&settings.sites, &request.host));
    if !listed && let Err(why) = ask(dir, request.session.as_deref(), &request.host, request.port) {
        let _ = refuse(&mut client, 403, &why);
        return;
    }
    let upstream = (request.host.as_str(), request.port)
        .to_socket_addrs()
        .ok()
        .and_then(|mut addresses| {
            addresses.find_map(|address| TcpStream::connect_timeout(&address, CONNECT_TIMEOUT).ok())
        });
    let Some(mut upstream) = upstream else {
        let _ = refuse(
            &mut client,
            502,
            &format!("{} could not be reached", request.host),
        );
        return;
    };
    let sent = if request.tunnel {
        client.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
    } else {
        upstream.write_all(&request.head)
    };
    if sent.is_err() || upstream.write_all(&rest).is_err() {
        return;
    }
    pipe(client, upstream);
}

/// What a session's user said about a site.
#[derive(Debug, Clone)]
enum Answer {
    /// Being asked; other connections to the site wait for the answer.
    Asking,
    /// For the rest of the session.
    Allowed,
    /// Not allowed, and why; asked again after a little while (connections made at once all get
    /// the same answer, a later try asks again).
    Refused(String, Instant),
}

/// How long a "no" stands for further connections to the site.
const REFUSED_FOR: Duration = Duration::from_secs(20);

/// The answers, by session and site, and a signal for when one comes.
type Answers = (Mutex<HashMap<(String, String), Answer>>, Condvar);

fn answers() -> &'static Answers {
    static ANSWERS: OnceLock<Answers> = OnceLock::new();
    ANSWERS.get_or_init(Default::default)
}

/// Whether `session`'s commands may reach `host`: asked once per session and site (connections
/// at the same time share the question), through the session's gate socket in `dir`.
fn ask(dir: &Path, session: Option<&str>, host: &str, port: u16) -> Result<(), String> {
    let Some(session) = session else {
        return Err(format!(
            "{host} is not an allowed site, and Pistachio couldn't tell which conversation to ask \
             (Settings → Sandboxing & Security → Internet)"
        ));
    };
    let key = (session.to_owned(), host.to_owned());
    let (lock, changed) = answers();
    let mut map = lock
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    loop {
        match map.get(&key) {
            Some(Answer::Allowed) => return Ok(()),
            Some(Answer::Refused(why, at)) if at.elapsed() < REFUSED_FOR => {
                return Err(why.clone());
            }
            Some(Answer::Asking) => {
                map = changed
                    .wait(map)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
            _ => break,
        }
    }
    map.insert(key.clone(), Answer::Asking);
    drop(map);
    let answer = ask_gate(dir, session, host, port);
    let mut map = lock
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    map.insert(
        key,
        match &answer {
            Ok(()) => Answer::Allowed,
            Err(why) => Answer::Refused(why.clone(), Instant::now()),
        },
    );
    changed.notify_all();
    answer
}

/// The gate sockets `session` (`pi-<pid>`) may listen on: in `dir`, or (for a long `dir`) in a
/// short folder of this user's, as the sandbox extension chooses.
fn gate_sockets(dir: &Path, session: &str) -> Vec<PathBuf> {
    let Some(pid) = session
        .strip_prefix("pi-")
        .filter(|pid| pid.bytes().all(|byte| byte.is_ascii_digit()))
    else {
        return Vec::new();
    };
    let name = format!("{pid}.sock");
    let mut sockets = vec![dir.join("gate").join(&name)];
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        if let Ok(meta) = std::fs::metadata(dir) {
            sockets.push(
                PathBuf::from("/tmp")
                    .join(format!("pistachio-gate-{}", meta.uid()))
                    .join(&name),
            );
        }
    }
    sockets
}

/// Asks the session's gate (and through it the user) about `host`.
#[cfg(unix)]
fn ask_gate(dir: &Path, session: &str, host: &str, port: u16) -> Result<(), String> {
    use std::os::unix::net::UnixStream;
    let Some(mut stream) = gate_sockets(dir, session)
        .iter()
        .find_map(|socket| UnixStream::connect(socket).ok())
    else {
        return Err(format!(
            "{host} is not an allowed site, and its conversation couldn't be asked (Settings → \
             Sandboxing & Security → Internet)"
        ));
    };
    let request = crate::gate::Request {
        tool: "site".to_owned(),
        args: vec![host.to_owned(), port.to_string()],
        cwd: PathBuf::from("/"),
        project: None,
        stdin: None,
        account: None,
    };
    let mut line = serde_json::to_string(&request).map_err(|error| error.to_string())?;
    line.push('\n');
    stream
        .write_all(line.as_bytes())
        .map_err(|error| error.to_string())?;
    let mut said = String::new();
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else { break };
        let Ok(frame) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if let Some(text) = frame["stderr"].as_str() {
            said.push_str(text);
        }
        if let Some(code) = frame["exit"].as_i64() {
            return if code == 0 {
                Ok(())
            } else {
                let said = said.trim().trim_start_matches("[Pistachio]").trim();
                Err(if said.is_empty() {
                    format!("The user didn't allow {host}.")
                } else {
                    said.to_owned()
                })
            };
        }
    }
    Err(format!("Asking about {host} ended without an answer."))
}

#[cfg(not(unix))]
fn ask_gate(_dir: &Path, _session: &str, host: &str, _port: u16) -> Result<(), String> {
    Err(format!("{host} is not an allowed site"))
}

/// Reads up to the end of the request head; returns it and what came after it.
fn read_head<S: Read>(client: &mut S) -> io::Result<(Vec<u8>, Vec<u8>)> {
    let mut buffer = Vec::with_capacity(1024);
    let mut chunk = [0u8; 4096];
    loop {
        let read = client.read(&mut chunk)?;
        if read == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(end) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            let rest = buffer.split_off(end + 4);
            return Ok((buffer, rest));
        }
        if buffer.len() > HEAD_LIMIT {
            return Err(io::ErrorKind::InvalidData.into());
        }
    }
}

fn refuse<S: Write>(client: &mut S, status: u16, why: &str) -> io::Result<()> {
    let reason = match status {
        400 => "Bad Request",
        403 => "Forbidden",
        _ => "Bad Gateway",
    };
    let body = format!("{why}\n");
    write!(
        client,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

/// Passes bytes both ways until both sides are done.
fn pipe<A: Duplex, B: Duplex>(a: A, b: B) {
    let (Ok(mut a_read), Ok(mut b_write)) = (a.try_clone(), b.try_clone()) else {
        return;
    };
    let forward = std::thread::spawn(move || {
        let _ = io::copy(&mut a_read, &mut b_write);
        b_write.close_write();
    });
    let (mut b_read, mut a_write) = (b, a);
    let _ = io::copy(&mut b_read, &mut a_write);
    a_write.close_write();
    let _ = forward.join();
}

/// Inside bubblewrap's own network: listens on the loopback `port` and passes each connection on
/// to the proxy's Unix `socket`, runs the command, and ends with it.
#[cfg(unix)]
pub fn bridge(
    socket: &Path,
    port: u16,
    program: &std::ffi::OsStr,
    args: &[std::ffi::OsString],
) -> i32 {
    use std::os::unix::net::UnixStream;
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("Pistachio's sandbox: the network bridge could not start: {error}");
            return 126;
        }
    };
    let socket = socket.to_path_buf();
    std::thread::spawn(move || {
        for client in listener.incoming().flatten() {
            let socket = socket.clone();
            std::thread::spawn(move || {
                if let Ok(proxy) = UnixStream::connect(&socket) {
                    pipe(client, proxy);
                }
            });
        }
    });
    match std::process::Command::new(program).args(args).status() {
        Ok(status) => status.code().unwrap_or(1),
        Err(error) => {
            eprintln!("{}: {error}", Path::new(program).display());
            127
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_where_requests_go() {
        let tunnel = parse_head(b"CONNECT github.com:443 HTTP/1.1\r\nHost: github.com:443\r\n\r\n")
            .expect("valid");
        assert_eq!(
            (tunnel.host.as_str(), tunnel.port, tunnel.tunnel),
            ("github.com", 443, true)
        );
        let plain = parse_head(
            b"GET http://Example.com:8080/a?b=1 HTTP/1.1\r\nHost: example.com\r\nProxy-Connection: keep-alive\r\n\r\n",
        )
        .expect("valid");
        assert_eq!((plain.host.as_str(), plain.port), ("example.com", 8080));
        assert_eq!(
            String::from_utf8(plain.head).expect("utf-8"),
            "GET /a?b=1 HTTP/1.1\r\nHost: example.com\r\n\r\n"
        );
        assert!(parse_head(b"GET /relative HTTP/1.1\r\n\r\n").is_err());
        assert_eq!(split_host("[::1]:8443", 443), Some(("::1".into(), 8443)));
    }

    /// The real thing on this computer: an allowed site is reached through the proxy, another
    /// one is refused.
    #[test]
    fn only_allowed_sites_get_through() {
        let dir = tempfile::tempdir().expect("temp dir");
        // A site on this computer, standing in for one on the Internet.
        let site = TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let site_port = site.local_addr().expect("address").port();
        std::thread::spawn(move || {
            for mut stream in site.incoming().flatten() {
                let _ = read_head(&mut stream);
                let _ = stream.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello",
                );
            }
        });
        let settings = Settings {
            network: crate::Network::Custom,
            sites: vec!["127.0.0.1".into()],
            ..Settings::default()
        };
        settings.save(dir.path()).expect("save");
        let endpoint = start(dir.path()).expect("proxy starts");
        assert_eq!(super::endpoint(dir.path()), Some(endpoint.clone()));

        let get = |target: &str| {
            let mut stream = TcpStream::connect(("127.0.0.1", endpoint.port)).expect("connect");
            write!(stream, "GET {target} HTTP/1.1\r\nHost: x\r\n\r\n").expect("write");
            let mut answer = String::new();
            let _ = stream.read_to_string(&mut answer);
            answer
        };
        let allowed = get(&format!("http://127.0.0.1:{site_port}/"));
        assert!(
            allowed.starts_with("HTTP/1.1 200") && allowed.ends_with("hello"),
            "{allowed}"
        );
        // Not listed, and no conversation named to ask.
        let refused = get(&format!("http://localhost:{site_port}/"));
        assert!(refused.starts_with("HTTP/1.1 403"), "{refused}");
        assert!(refused.contains("localhost is not an allowed site"));
    }

    #[test]
    fn the_proxy_names_the_session() {
        let endpoint = Endpoint {
            port: 9,
            socket: None,
        };
        let env = endpoint.env_for(session_of(Path::new("/x/gate/4242.sock")).as_deref());
        assert!(env.contains(&("HTTPS_PROXY", "http://pi-4242:x@127.0.0.1:9".to_owned())));
        assert_eq!(session_of(Path::new("/x/gate/other.sock")), None);
        let head = b"CONNECT a.dev:443 HTTP/1.1\r\nProxy-Authorization: Basic cGktNDI0Mjp4\r\n\r\n";
        assert_eq!(
            parse_head(head).expect("valid").session.as_deref(),
            Some("pi-4242")
        );
        assert_eq!(base64_decode("aGk="), Some(b"hi".to_vec()));
    }

    /// With nothing listed, a site is asked about in the session's conversation (through its
    /// gate socket): once for connections that come together, and the answer stands.
    #[cfg(unix)]
    #[test]
    fn unlisted_sites_are_asked_in_their_conversation() {
        use std::os::unix::net::UnixListener;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let dir = tempfile::tempdir().expect("temp dir");
        let site = TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let site_port = site.local_addr().expect("address").port();
        std::thread::spawn(move || {
            for mut stream in site.incoming().flatten() {
                let _ = read_head(&mut stream);
                let _ = stream.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello",
                );
            }
        });
        Settings::default().save(dir.path()).expect("save");
        // Two conversations: one says yes (slowly), the other no.
        std::fs::create_dir_all(dir.path().join("gate")).expect("mkdir");
        let asked = Arc::new(AtomicUsize::new(0));
        for (pid, yes) in [(4242, true), (4343, false)] {
            let gate =
                UnixListener::bind(dir.path().join(format!("gate/{pid}.sock"))).expect("bind");
            let asked = asked.clone();
            std::thread::spawn(move || {
                for mut stream in gate.incoming().flatten() {
                    asked.fetch_add(1, Ordering::SeqCst);
                    let mut line = String::new();
                    let _ = BufReader::new(stream.try_clone().expect("clone")).read_line(&mut line);
                    let request: crate::gate::Request =
                        serde_json::from_str(&line).expect("request");
                    assert_eq!(request.tool, "site");
                    assert_eq!(request.args[0], "localhost");
                    std::thread::sleep(Duration::from_millis(200));
                    let answer = if yes {
                        "{\"exit\":0}\n".to_owned()
                    } else {
                        "{\"stderr\":\"[Pistachio] The user didn't allow it.\\n\"}\n{\"exit\":1}\n"
                            .to_owned()
                    };
                    let _ = stream.write_all(answer.as_bytes());
                }
            });
        }
        let endpoint = start(dir.path()).expect("proxy starts");
        let get = move |credentials: &str| {
            let mut stream = TcpStream::connect(("127.0.0.1", endpoint.port)).expect("connect");
            write!(
                stream,
                "GET http://localhost:{site_port}/ HTTP/1.1\r\nHost: x\r\n\
                 Proxy-Authorization: Basic {credentials}\r\n\r\n"
            )
            .expect("write");
            let mut answer = String::new();
            let _ = stream.read_to_string(&mut answer);
            answer
        };
        let together: Vec<_> = (0..3)
            .map(|_| std::thread::spawn(move || get("cGktNDI0Mjp4")))
            .collect();
        for answer in together {
            let answer = answer.join().expect("thread");
            assert!(answer.ends_with("hello"), "{answer}");
        }
        assert_eq!(asked.load(Ordering::SeqCst), 1);
        assert!(get("cGktNDI0Mjp4").ends_with("hello"));
        assert_eq!(asked.load(Ordering::SeqCst), 1);
        let refused = get("cGktNDM0Mzp4");
        assert!(refused.starts_with("HTTP/1.1 403"), "{refused}");
        assert!(refused.contains("The user didn't allow it."), "{refused}");
    }
}
