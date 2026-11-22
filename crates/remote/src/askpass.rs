//! In-app password and passphrase prompts for `ssh`, through OpenSSH's `SSH_ASKPASS`.
//!
//! When the app connects interactively it starts ssh with `SSH_ASKPASS` pointing at its own
//! executable and `SSH_ASKPASS_REQUIRE=force`. ssh then runs that executable with the prompt
//! as its argument instead of reading from a terminal; [`run_client_if_requested`] (called
//! first thing in `main`) recognises this, hands the prompt to the running app over a private
//! Unix socket, and prints the user's answer for ssh.
//!
//! Wire format, client to server: the token, a NUL byte, the prompt. Server to client: `1`
//! and the answer, or `0` when the user cancelled. The socket lives in the private (0700)
//! control-socket directory and every request must carry the per-process random token.

use std::ffi::OsString;
use std::fs;
use std::io::{self, Read as _, Write as _};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

const SOCKET_ENV: &str = "PI_GUI_ASKPASS_SOCKET";
const TOKEN_ENV: &str = "PI_GUI_ASKPASS_TOKEN";
/// How long a prompt may stay unanswered before ssh is told it was cancelled.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(300);

/// A question from ssh, waiting for the user.
#[derive(Debug)]
pub struct Prompt {
    /// What ssh asks, e.g. `Enter passphrase for key '/Users/me/.ssh/id_ed25519':`.
    pub text: String,
    reply: mpsc::Sender<Option<String>>,
}

impl Prompt {
    /// Whether the answer is a secret (password, passphrase, PIN) rather than a yes/no.
    pub fn is_secret(&self) -> bool {
        let text = self.text.to_lowercase();
        !(text.contains("(yes/no") || text.contains("continue connecting"))
    }

    /// Sends the answer to ssh; `None` cancels.
    pub fn answer(self, answer: Option<String>) {
        let _ = self.reply.send(answer);
    }
}

/// Serves askpass requests for the app's lifetime; dropping it removes the socket.
pub struct AskpassServer {
    socket: PathBuf,
    token: String,
}

impl AskpassServer {
    /// Listens on `<dir>/askpass-<pid>.sock`; every prompt is sent to `prompts`.
    pub fn start(dir: &Path, prompts: async_channel::Sender<Prompt>) -> io::Result<Self> {
        // The socket answers ssh's password prompts: its folder must be ours alone.
        crate::ssh::prepare_private_dir(dir)?;
        let socket = dir.join(format!("askpass-{}.sock", std::process::id()));
        let _ = fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket)?;
        let token = random_token();
        let expected = token.clone();
        thread::Builder::new()
            .name("ssh-askpass".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    let Ok(stream) = stream else { continue };
                    let prompts = prompts.clone();
                    let expected = expected.clone();
                    thread::spawn(move || serve(stream, &expected, &prompts));
                }
            })?;
        Ok(Self { socket, token })
    }

    /// Environment for an `ssh` that should ask through this server. `exe` is the app's own
    /// executable (it answers as the askpass program).
    pub fn env(&self, exe: &Path) -> Vec<(OsString, OsString)> {
        let mut env = vec![
            ("SSH_ASKPASS".into(), exe.into()),
            ("SSH_ASKPASS_REQUIRE".into(), "force".into()),
            (SOCKET_ENV.into(), self.socket.clone().into()),
            (TOKEN_ENV.into(), self.token.clone().into()),
        ];
        // OpenSSH before 8.4 only uses askpass when DISPLAY is set.
        if std::env::var_os("DISPLAY").is_none() {
            env.push(("DISPLAY".into(), ":0".into()));
        }
        env
    }
}

impl Drop for AskpassServer {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.socket);
    }
}

fn serve(mut stream: UnixStream, expected: &str, prompts: &async_channel::Sender<Prompt>) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut request = Vec::new();
    if stream.read_to_end(&mut request).is_err() {
        return;
    }
    let Some(split) = request.iter().position(|&byte| byte == 0) else {
        return;
    };
    if request[..split] != *expected.as_bytes() {
        log::warn!("ignoring an askpass request with a wrong token");
        return;
    }
    let text = String::from_utf8_lossy(&request[split + 1..])
        .trim()
        .to_owned();
    let (reply, answer) = mpsc::channel();
    if prompts.send_blocking(Prompt { text, reply }).is_err() {
        let _ = stream.write_all(b"0");
        return;
    }
    let response = match answer.recv_timeout(ANSWER_TIMEOUT) {
        Ok(Some(answer)) => format!("1{answer}"),
        _ => "0".to_owned(),
    };
    let _ = stream.write_all(response.as_bytes());
}

/// If this process was started by ssh as its askpass program, asks the app and returns the
/// exit code to end with; otherwise `None` (start the app normally).
pub fn run_client_if_requested() -> Option<i32> {
    let socket = std::env::var_os(SOCKET_ENV)?;
    let token = std::env::var(TOKEN_ENV).ok()?;
    let prompt: Vec<String> = std::env::args().skip(1).collect();
    Some(match ask(Path::new(&socket), &token, &prompt.join(" ")) {
        Ok(Some(answer)) => {
            let mut stdout = io::stdout();
            let _ = writeln!(stdout, "{answer}");
            let _ = stdout.flush();
            0
        }
        Ok(None) => 1,
        Err(error) => {
            eprintln!("pi-gui askpass: {error}");
            1
        }
    })
}

/// The client side: sends `prompt` and waits for the answer (`None`: cancelled).
fn ask(socket: &Path, token: &str, prompt: &str) -> io::Result<Option<String>> {
    let mut stream = UnixStream::connect(socket)?;
    stream.write_all(token.as_bytes())?;
    stream.write_all(&[0])?;
    stream.write_all(prompt.as_bytes())?;
    stream.shutdown(std::net::Shutdown::Write)?;
    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    Ok(response.strip_prefix('1').map(str::to_owned))
}

/// 32 hex characters from the OS random source (falls back to time and pid).
fn random_token() -> String {
    let mut bytes = [0u8; 16];
    let read = fs::File::open("/dev/urandom").and_then(|mut file| file.read_exact(&mut bytes));
    if read.is_err() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |time| time.as_nanos());
        bytes = (nanos ^ (u128::from(std::process::id()) << 64)).to_le_bytes();
    }
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(32), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_cancel_and_wrong_token() {
        let dir = tempfile::Builder::new()
            .prefix("ap")
            .tempdir_in("/tmp")
            .unwrap();
        let (sender, prompts) = async_channel::unbounded();
        let server = AskpassServer::start(dir.path(), sender).unwrap();
        let answerer = thread::spawn(move || {
            let first = prompts.recv_blocking().unwrap();
            assert_eq!(first.text, "Enter passphrase for key 'k':");
            assert!(first.is_secret());
            first.answer(Some("s3cret pass".into()));
            let second = prompts.recv_blocking().unwrap();
            assert!(!second.is_secret());
            second.answer(None);
        });
        assert_eq!(
            ask(
                &server.socket,
                &server.token,
                "Enter passphrase for key 'k':"
            )
            .unwrap(),
            Some("s3cret pass".into())
        );
        assert_eq!(
            ask(
                &server.socket,
                &server.token,
                "Are you sure you want to continue connecting (yes/no)?"
            )
            .unwrap(),
            None
        );
        answerer.join().unwrap();
        // A request without the token gets no answer.
        assert_eq!(ask(&server.socket, "wrong", "x").unwrap(), None);
        let socket = server.socket.clone();
        drop(server);
        assert!(!socket.exists());
    }
}
