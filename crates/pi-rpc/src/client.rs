//! Command/response correlation over any pair of byte streams.
//!
//! The client owns two OS threads: a reader that decodes stdout records and a writer
//! that drains an unbounded queue into stdin, so callers (the UI thread) never block
//! on pipe backpressure. Responses are matched to requests by a generated `id`;
//! everything else is forwarded as a [`ClientEvent`].

use std::collections::HashMap;
use std::io::{BufReader, Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

use futures::channel::oneshot;
use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::error::RpcError;
use crate::framing::{JsonlReader, write_record};
use crate::protocol::{
    AgentMessage, BashResult, ClearedQueue, Command, CommandEnvelope, Disposition, Event,
    ExtensionUiAnswer, ExtensionUiRequest, ExtensionUiResponse, ForkMessage, ImageContent,
    Incoming, Model, Response, SessionState, SessionStats, SlashCommand, StreamingBehavior,
    ThinkingLevel,
};

/// Everything pi sends that is not a response to one of our commands.
#[derive(Debug, Clone, PartialEq)]
pub enum ClientEvent {
    Event(Box<Event>),
    ExtensionUi(ExtensionUiRequest),
    /// A stdout line that could not be decoded. pi keeps running; we log and skip.
    ProtocolError {
        line: String,
        error: String,
    },
    /// stdout closed: the process exited or crashed. No further events follow.
    Disconnected,
}

/// Cheap-to-clone handle for sending commands to one pi process.
#[derive(Clone)]
pub struct PiClient {
    shared: Arc<Shared>,
}

struct Shared {
    outgoing: async_channel::Sender<Vec<u8>>,
    pending: Mutex<HashMap<String, oneshot::Sender<Response>>>,
    next_id: AtomicU64,
    connected: AtomicBool,
}

impl Shared {
    fn take_pending(&self, id: &str) -> Option<oneshot::Sender<Response>> {
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(id)
    }

    fn disconnect(&self) {
        self.connected.store(false, Ordering::SeqCst);
        self.outgoing.close();
        // Dropping the senders wakes every waiting request with `Disconnected`.
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }
}

impl PiClient {
    /// Starts the reader and writer threads over `reader` (pi's stdout) and
    /// `writer` (pi's stdin).
    pub fn connect<R, W>(reader: R, writer: W) -> (Self, async_channel::Receiver<ClientEvent>)
    where
        R: Read + Send + 'static,
        W: Write + Send + 'static,
    {
        let (outgoing, outgoing_rx) = async_channel::unbounded::<Vec<u8>>();
        let (events, events_rx) = async_channel::unbounded();
        let shared = Arc::new(Shared {
            outgoing,
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            connected: AtomicBool::new(true),
        });

        let writer_shared = Arc::clone(&shared);
        thread::Builder::new()
            .name("pi-rpc-writer".into())
            .spawn(move || {
                let mut writer = writer;
                while let Ok(line) = outgoing_rx.recv_blocking() {
                    if let Err(error) = writer.write_all(&line).and_then(|()| writer.flush()) {
                        log::warn!("pi stdin closed: {error}");
                        break;
                    }
                }
                drop(writer);
                writer_shared.connected.store(false, Ordering::SeqCst);
            })
            .expect("spawn pi-rpc writer thread");

        let reader_shared = Arc::clone(&shared);
        thread::Builder::new()
            .name("pi-rpc-reader".into())
            .spawn(move || read_loop(BufReader::new(reader), &reader_shared, &events))
            .expect("spawn pi-rpc reader thread");

        (Self { shared }, events_rx)
    }

    pub fn is_connected(&self) -> bool {
        self.shared.connected.load(Ordering::SeqCst)
    }

    /// Closes pi's stdin, which asks pi for an orderly shutdown.
    pub fn close(&self) {
        self.shared.outgoing.close();
    }

    /// A new request id, for [`bash`](Self::bash): events about the command carry it.
    pub fn next_id(&self) -> String {
        format!(
            "gui-{}",
            self.shared.next_id.fetch_add(1, Ordering::Relaxed)
        )
    }

    /// Sends `command` and waits for its response. `success: false` becomes
    /// [`RpcError::Rejected`].
    pub async fn request(&self, command: Command) -> Result<Response, RpcError> {
        self.request_as(self.next_id(), command).await
    }

    /// [`request`](Self::request) with an `id` from [`next_id`](Self::next_id).
    async fn request_as(&self, id: String, command: Command) -> Result<Response, RpcError> {
        let (sender, receiver) = oneshot::channel();
        self.shared
            .pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id.clone(), sender);

        // Checked after registering so a concurrent disconnect cannot strand us.
        if !self.is_connected() {
            self.shared.take_pending(&id);
            return Err(RpcError::Disconnected);
        }

        let mut line = Vec::new();
        write_record(
            &mut line,
            &CommandEnvelope {
                id: Some(&id),
                command: &command,
            },
        )?;
        if self.shared.outgoing.try_send(line).is_err() {
            self.shared.take_pending(&id);
            return Err(RpcError::Disconnected);
        }

        let response = receiver.await.map_err(|_| RpcError::Disconnected)?;
        if response.success {
            Ok(response)
        } else {
            Err(RpcError::Rejected {
                command: response.command,
                message: response.error.unwrap_or_default(),
            })
        }
    }

    /// Like [`request`](Self::request), decoding `data` into `T`.
    pub async fn call<T: DeserializeOwned>(&self, command: Command) -> Result<T, RpcError> {
        decode(self.request(command).await?)
    }

    /// Answers an extension dialog. Fire-and-forget: pi sends no response.
    pub fn answer_extension_ui(
        &self,
        id: &str,
        answer: &ExtensionUiAnswer,
    ) -> Result<(), RpcError> {
        let mut line = Vec::new();
        write_record(&mut line, &ExtensionUiResponse::new(id, answer))?;
        self.shared
            .outgoing
            .try_send(line)
            .map_err(|_| RpcError::Disconnected)
    }

    // ---- typed helpers -------------------------------------------------------

    /// Sends a prompt. While the agent streams, `behavior` must say how to queue it.
    pub async fn prompt(
        &self,
        message: String,
        images: Vec<ImageContent>,
        behavior: Option<StreamingBehavior>,
    ) -> Result<Disposition, RpcError> {
        #[derive(Deserialize)]
        struct Data {
            disposition: Disposition,
        }
        let command = Command::Prompt {
            message,
            images,
            streaming_behavior: behavior,
        };
        Ok(self.call::<Data>(command).await?.disposition)
    }

    pub async fn abort(&self) -> Result<(), RpcError> {
        self.request(Command::Abort).await.map(drop)
    }

    /// Removes queued steering/follow-up messages and returns their texts.
    pub async fn clear_queue(&self) -> Result<ClearedQueue, RpcError> {
        self.call(Command::ClearQueue).await
    }

    pub async fn get_state(&self) -> Result<SessionState, RpcError> {
        self.call(Command::GetState).await
    }

    pub async fn get_session_stats(&self) -> Result<SessionStats, RpcError> {
        self.call(Command::GetSessionStats).await
    }

    pub async fn get_messages(&self) -> Result<Vec<AgentMessage>, RpcError> {
        #[derive(Deserialize)]
        struct Data {
            messages: Vec<AgentMessage>,
        }
        Ok(self.call::<Data>(Command::GetMessages).await?.messages)
    }

    pub async fn get_available_models(&self) -> Result<Vec<Model>, RpcError> {
        #[derive(Deserialize)]
        struct Data {
            models: Vec<Model>,
        }
        Ok(self.call::<Data>(Command::GetAvailableModels).await?.models)
    }

    pub async fn get_available_thinking_levels(&self) -> Result<Vec<ThinkingLevel>, RpcError> {
        #[derive(Deserialize)]
        struct Data {
            levels: Vec<ThinkingLevel>,
        }
        Ok(self
            .call::<Data>(Command::GetAvailableThinkingLevels)
            .await?
            .levels)
    }

    pub async fn set_model(&self, provider: String, model_id: String) -> Result<Model, RpcError> {
        self.call(Command::SetModel { provider, model_id }).await
    }

    pub async fn set_thinking_level(&self, level: ThinkingLevel) -> Result<(), RpcError> {
        self.request(Command::SetThinkingLevel { level })
            .await
            .map(drop)
    }

    /// Starts a fresh session. Returns `false` if an extension cancelled it.
    pub async fn new_session(&self) -> Result<bool, RpcError> {
        self.call::<Cancellable>(Command::NewSession {
            parent_session: None,
        })
        .await
        .map(|data| !data.cancelled)
    }

    /// Loads another session file. Returns `false` if an extension cancelled it.
    pub async fn switch_session(&self, session_path: String) -> Result<bool, RpcError> {
        self.call::<Cancellable>(Command::SwitchSession { session_path })
            .await
            .map(|data| !data.cancelled)
    }

    pub async fn set_session_name(&self, name: String) -> Result<(), RpcError> {
        self.request(Command::SetSessionName { name })
            .await
            .map(drop)
    }

    pub async fn compact(&self, custom_instructions: Option<String>) -> Result<(), RpcError> {
        self.request(Command::Compact {
            custom_instructions,
        })
        .await
        .map(drop)
    }

    pub async fn get_commands(&self) -> Result<Vec<SlashCommand>, RpcError> {
        #[derive(Deserialize)]
        struct Data {
            commands: Vec<SlashCommand>,
        }
        Ok(self.call::<Data>(Command::GetCommands).await?.commands)
    }

    pub async fn get_fork_messages(&self) -> Result<Vec<ForkMessage>, RpcError> {
        #[derive(Deserialize)]
        struct Data {
            messages: Vec<ForkMessage>,
        }
        Ok(self.call::<Data>(Command::GetForkMessages).await?.messages)
    }

    /// Forks from a user message. Returns that message's text unless cancelled.
    pub async fn fork(&self, entry_id: String) -> Result<Option<String>, RpcError> {
        #[derive(Deserialize)]
        struct Data {
            #[serde(default)]
            text: Option<String>,
            #[serde(default)]
            cancelled: bool,
        }
        let data: Data = self.call(Command::Fork { entry_id }).await?;
        Ok(if data.cancelled { None } else { data.text })
    }

    /// Writes the session as HTML (to `output_path`, or pi's default) and returns the path.
    pub async fn export_html(&self, output_path: Option<String>) -> Result<String, RpcError> {
        #[derive(Deserialize)]
        struct Data {
            path: String,
        }
        Ok(self
            .call::<Data>(Command::ExportHtml { output_path })
            .await?
            .path)
    }

    /// Duplicates the active branch into a new session. `false` if cancelled.
    pub async fn clone_session(&self) -> Result<bool, RpcError> {
        self.call::<Cancellable>(Command::Clone)
            .await
            .map(|data| !data.cancelled)
    }

    /// Runs a shell command. While it runs, [`Event::BashExecutionUpdate`]s with `id` (from
    /// [`next_id`](Self::next_id)) bring its output.
    pub async fn bash(
        &self,
        id: String,
        command: String,
        exclude_from_context: bool,
    ) -> Result<BashResult, RpcError> {
        let command = Command::Bash {
            command,
            exclude_from_context,
        };
        decode(self.request_as(id, command).await?)
    }

    /// Stops the shell commands running through [`bash`](Self::bash) ([`abort`](Self::abort)
    /// leaves them alone).
    pub async fn abort_bash(&self) -> Result<(), RpcError> {
        self.request(Command::AbortBash).await.map(drop)
    }
}

/// `response`'s data as a `T`.
fn decode<T: DeserializeOwned>(response: Response) -> Result<T, RpcError> {
    serde_json::from_value(response.data.unwrap_or_default()).map_err(|source| RpcError::Decode {
        command: response.command,
        source,
    })
}

#[derive(Deserialize)]
struct Cancellable {
    #[serde(default)]
    cancelled: bool,
}

fn read_loop<R: Read>(
    reader: BufReader<R>,
    shared: &Shared,
    events: &async_channel::Sender<ClientEvent>,
) {
    let mut lines = JsonlReader::new(reader);
    loop {
        let record = match lines.next_record() {
            Ok(Some(record)) => record,
            Ok(None) => break,
            Err(error) => {
                log::warn!("pi stdout read failed: {error}");
                break;
            }
        };
        let forwarded = match Incoming::from_slice(record) {
            Ok(Incoming::Response(response)) => {
                match response
                    .id
                    .as_deref()
                    .and_then(|id| shared.take_pending(id))
                {
                    // The requester may have given up; that's fine.
                    Some(waiter) => drop(waiter.send(response)),
                    None => log::debug!("unsolicited pi response: {response:?}"),
                }
                continue;
            }
            Ok(Incoming::ExtensionUi(request)) => ClientEvent::ExtensionUi(request),
            Ok(Incoming::Event(event)) => ClientEvent::Event(event),
            Err(error) => ClientEvent::ProtocolError {
                line: String::from_utf8_lossy(record).into_owned(),
                error: error.to_string(),
            },
        };
        if events.send_blocking(forwarded).is_err() {
            // Nobody listens any more; keep draining so pi never stalls on stdout.
        }
    }
    shared.disconnect();
    let _ = events.send_blocking(ClientEvent::Disconnected);
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::executor::block_on;
    use std::io::{BufRead, Write, pipe};

    /// A scripted fake pi: replies to each command via `respond`.
    fn fake_pi(
        respond: impl Fn(&serde_json::Value) -> Vec<serde_json::Value> + Send + 'static,
    ) -> (PiClient, async_channel::Receiver<ClientEvent>) {
        let (stdin_reader, stdin_writer) = pipe().unwrap();
        let (stdout_reader, mut stdout_writer) = pipe().unwrap();
        thread::spawn(move || {
            for line in BufReader::new(stdin_reader).lines() {
                let command: serde_json::Value = serde_json::from_str(&line.unwrap()).unwrap();
                for record in respond(&command) {
                    writeln!(stdout_writer, "{record}").unwrap();
                }
            }
            // stdin closed -> pi exits, closing stdout.
        });
        PiClient::connect(stdout_reader, stdin_writer)
    }

    #[test]
    fn correlates_responses_and_forwards_events() {
        let (client, events) = fake_pi(|command| {
            let id = command["id"].clone();
            vec![
                serde_json::json!({"type":"agent_start"}),
                serde_json::json!({
                    "id": id, "type":"response","command":"get_available_thinking_levels",
                    "success":true,"data":{"levels":["off","high"]}
                }),
            ]
        });

        let levels = block_on(client.get_available_thinking_levels()).unwrap();
        assert_eq!(levels, vec![ThinkingLevel::Off, ThinkingLevel::High]);
        assert_eq!(
            block_on(events.recv()).unwrap(),
            ClientEvent::Event(Box::new(Event::AgentStart))
        );
    }

    #[test]
    fn rejected_commands_surface_pi_error() {
        let (client, _events) = fake_pi(|command| {
            vec![serde_json::json!({
                "id": command["id"], "type":"response","command":"set_model",
                "success":false,"error":"Model not found: x/y"
            })]
        });
        let error = block_on(client.set_model("x".into(), "y".into())).unwrap_err();
        assert!(
            matches!(&error, RpcError::Rejected { message, .. } if message == "Model not found: x/y"),
            "{error:?}"
        );
    }

    #[test]
    fn closing_disconnects_pending_and_future_requests() {
        let (client, events) = fake_pi(|_| vec![]);
        client.close();
        // Drain until the reader observes EOF.
        while let Ok(event) = block_on(events.recv()) {
            if event == ClientEvent::Disconnected {
                break;
            }
        }
        assert!(!client.is_connected());
        assert!(matches!(
            block_on(client.get_state()),
            Err(RpcError::Disconnected)
        ));
    }

    #[test]
    fn bad_lines_become_protocol_errors() {
        let (client, events) = fake_pi(|command| {
            vec![
                serde_json::json!("not an object"),
                serde_json::json!({"id": command["id"], "type":"response","command":"abort","success":true}),
            ]
        });
        block_on(client.abort()).unwrap();
        assert!(matches!(
            block_on(events.recv()).unwrap(),
            ClientEvent::ProtocolError { .. }
        ));
    }
}
