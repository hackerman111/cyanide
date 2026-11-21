use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener as StdUnixListener, UnixStream as StdUnixStream};
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;
use std::time::Duration;

use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader as TokioBufReader};
use tokio::net::UnixListener;
use tokio::sync::{oneshot, watch};

use crate::state::{AudioSource, Command, MicAction, RecordAction, Reply, StreamMode, VideoSource};

const MAX_COMMAND_LINE_BYTES: u64 = 8192;

#[derive(Debug, Error)]
pub enum IpcError {
    #[error("cyanide daemon is already running")]
    AlreadyRunning,
    #[error("IPC I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("IPC JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("IPC runtime error: {0}")]
    Runtime(String),
}

pub struct IpcRequest {
    pub command: Command,
    pub reply: oneshot::Sender<Reply>,
}

pub struct IpcServerHandle {
    socket_path: PathBuf,
    shutdown: watch::Sender<bool>,
    thread: Option<JoinHandle<()>>,
}

impl IpcServerHandle {
    pub fn start(
        socket_path: PathBuf,
        tx: async_channel::Sender<IpcRequest>,
    ) -> Result<Self, IpcError> {
        Self::start_inner(socket_path, tx, false)
    }

    pub fn start_with_signals(
        socket_path: PathBuf,
        tx: async_channel::Sender<IpcRequest>,
    ) -> Result<Self, IpcError> {
        Self::start_inner(socket_path, tx, true)
    }

    fn start_inner(
        socket_path: PathBuf,
        tx: async_channel::Sender<IpcRequest>,
        watch_signals: bool,
    ) -> Result<Self, IpcError> {
        if socket_path.exists() {
            if StdUnixStream::connect(&socket_path).is_ok() {
                return Err(IpcError::AlreadyRunning);
            }
            let _ = std::fs::remove_file(&socket_path);
        }
        if let Some(parent) = socket_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let std_listener = StdUnixListener::bind(&socket_path)?;
        let _ = std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600));
        std_listener.set_nonblocking(true)?;

        let (shutdown, shutdown_rx) = watch::channel(false);
        let thread = std::thread::Builder::new()
            .name("cyanide-ipc".into())
            .spawn(move || {
                let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                    .enable_io()
                    .enable_time()
                    .build()
                else {
                    return;
                };
                rt.block_on(async move {
                    let Ok(listener) = UnixListener::from_std(std_listener) else {
                        return;
                    };
                    if watch_signals {
                        spawn_unix_signal_forwarder(tx.clone(), shutdown_rx.clone());
                    }
                    serve_loop(listener, tx, shutdown_rx).await;
                });
                rt.shutdown_timeout(Duration::from_millis(500));
            })
            .map_err(|e| IpcError::Runtime(e.to_string()))?;

        Ok(Self {
            socket_path,
            shutdown,
            thread: Some(thread),
        })
    }

    pub fn shutdown(mut self) {
        self.stop_inner();
    }

    fn stop_inner(&mut self) {
        let _ = self.shutdown.send(true);
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

impl Drop for IpcServerHandle {
    fn drop(&mut self) {
        self.stop_inner();
    }
}

pub fn default_socket_path() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR").filter(|v| !v.is_empty()) {
        return PathBuf::from(dir).join("cyanide.sock");
    }
    let user = std::env::var("USER").unwrap_or_else(|_| "user".into());
    std::env::temp_dir().join(format!("cyanide-{user}.sock"))
}

pub fn parse_command_line(line: &str) -> Result<Command, String> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Err("empty command".into());
    }
    if trimmed.starts_with('{') {
        return serde_json::from_str::<Command>(trimmed)
            .map_err(|e| format!("invalid JSON command: {e}"));
    }

    let mut parts = trimmed.split_whitespace();
    let head = parts.next().unwrap_or_default().to_ascii_lowercase();
    let arg = parts.next().map(str::to_ascii_lowercase);
    if parts.next().is_some() {
        return Err(format!("unexpected extra arguments in '{trimmed}'"));
    }

    match (head.as_str(), arg.as_deref()) {
        ("toggle", None) => Ok(Command::Toggle),
        ("show", None) => Ok(Command::Show),
        ("hide", None) => Ok(Command::Hide),
        ("status", None) => Ok(Command::Status),
        ("quit", None) => Ok(Command::Quit),
        ("mode", Some("loop")) => Ok(Command::SetMode(StreamMode::Loop)),
        ("mode", Some("live")) => Ok(Command::SetMode(StreamMode::Live)),
        ("video", Some("loop")) => Ok(Command::SetVideo(VideoSource::Loop)),
        ("video", Some("live")) => Ok(Command::SetVideo(VideoSource::Live)),
        ("audio", Some("file")) => Ok(Command::SetAudio(AudioSource::File)),
        ("audio", Some("mic")) => Ok(Command::SetAudio(AudioSource::Mic)),
        ("audio", Some("mix")) => Ok(Command::SetAudio(AudioSource::Mix)),
        ("audio", Some("silence")) => Ok(Command::SetAudio(AudioSource::Silence)),
        ("mic", Some("on")) => Ok(Command::SetMic(MicAction::On)),
        ("mic", Some("off")) => Ok(Command::SetMic(MicAction::Off)),
        ("mic", Some("toggle")) => Ok(Command::SetMic(MicAction::Toggle)),
        ("record", Some("start")) => Ok(Command::Record(RecordAction::Start)),
        ("record", Some("stop")) => Ok(Command::Record(RecordAction::Stop)),
        ("record", Some("toggle")) => Ok(Command::Record(RecordAction::Toggle)),
        _ => Err(format!("unknown command '{trimmed}'")),
    }
}

pub fn send_command(socket_path: &Path, command: &Command) -> Result<Reply, IpcError> {
    let mut stream = StdUnixStream::connect(socket_path)?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;

    let mut payload = serde_json::to_string(command)?;
    payload.push('\n');
    stream.write_all(payload.as_bytes())?;
    stream.flush()?;

    let mut line = String::new();
    let mut reader = BufReader::new(stream);
    reader.read_line(&mut line)?;
    let reply: Reply = serde_json::from_str(line.trim())?;
    Ok(reply)
}

fn spawn_unix_signal_forwarder(
    tx: async_channel::Sender<IpcRequest>,
    mut shutdown: watch::Receiver<bool>,
) {
    use tokio::signal::unix::{SignalKind, signal};

    tokio::spawn(async move {
        let Ok(mut sigint) = signal(SignalKind::interrupt()) else {
            return;
        };
        let Ok(mut sigterm) = signal(SignalKind::terminate()) else {
            return;
        };
        tokio::select! {
            _ = shutdown.wait_for(|&stop| stop) => return,
            _ = sigint.recv() => {},
            _ = sigterm.recv() => {},
        }
        let (reply_tx, _reply_rx) = oneshot::channel();
        let _ = tx
            .send(IpcRequest {
                command: Command::Quit,
                reply: reply_tx,
            })
            .await;
    });
}

async fn serve_loop(
    listener: UnixListener,
    tx: async_channel::Sender<IpcRequest>,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            _ = shutdown.wait_for(|&stop| stop) => break,
            accepted = listener.accept() => {
                let Ok((stream, _)) = accepted else {
                    break;
                };
                let tx_clone = tx.clone();
                tokio::spawn(async move {
                    handle_client(stream, tx_clone).await;
                });
            }
        }
    }
}

async fn handle_client(stream: tokio::net::UnixStream, tx: async_channel::Sender<IpcRequest>) {
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = TokioBufReader::new(read_half.take(MAX_COMMAND_LINE_BYTES));
    let mut line = String::new();
    if reader.read_line(&mut line).await.is_err() {
        return;
    }

    let reply = match parse_command_line(&line) {
        Ok(command) => {
            let (reply_tx, reply_rx) = oneshot::channel();
            if tx
                .send(IpcRequest {
                    command,
                    reply: reply_tx,
                })
                .await
                .is_err()
            {
                Reply::Error {
                    message: "daemon is shutting down".into(),
                }
            } else {
                match tokio::time::timeout(Duration::from_secs(3), reply_rx).await {
                    Ok(Ok(r)) => r,
                    Ok(Err(_)) => Reply::Error {
                        message: "daemon dropped request".into(),
                    },
                    Err(_) => Reply::Error {
                        message: "daemon timed out".into(),
                    },
                }
            }
        }
        Err(message) => Reply::Error { message },
    };

    if let Ok(mut encoded) = serde_json::to_string(&reply) {
        encoded.push('\n');
        let _ = write_half.write_all(encoded.as_bytes()).await;
        let _ = write_half.flush().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_command_line_accepts_text_and_json() {
        assert_eq!(parse_command_line("toggle").unwrap(), Command::Toggle);
        assert_eq!(
            parse_command_line("mode loop").unwrap(),
            Command::SetMode(StreamMode::Loop)
        );
        assert_eq!(
            parse_command_line("video live").unwrap(),
            Command::SetVideo(VideoSource::Live)
        );
        assert_eq!(
            parse_command_line("audio mix").unwrap(),
            Command::SetAudio(AudioSource::Mix)
        );
        assert_eq!(
            parse_command_line("mic off").unwrap(),
            Command::SetMic(MicAction::Off)
        );
        assert_eq!(
            parse_command_line("record toggle").unwrap(),
            Command::Record(RecordAction::Toggle)
        );
        assert_eq!(parse_command_line("show").unwrap(), Command::Show);
        let json = serde_json::to_string(&Command::Status).unwrap();
        assert_eq!(parse_command_line(&json).unwrap(), Command::Status);
        assert!(parse_command_line("bogus").is_err());
    }

    #[test]
    fn stale_socket_is_replaced_on_bind_and_roundtrips_commands() {
        let sock = std::env::temp_dir().join(format!("cyanide-ipc-{}.sock", std::process::id()));
        std::fs::write(&sock, b"stale").unwrap();

        let (tx, rx) = async_channel::bounded::<IpcRequest>(8);
        let server = IpcServerHandle::start(sock.clone(), tx).unwrap();

        let responder = std::thread::spawn(move || {
            let req = rx.recv_blocking().unwrap();
            assert_eq!(req.command, Command::Toggle);
            let _ = req.reply.send(Reply::Ok {
                message: "toggled".into(),
            });
        });

        let reply = send_command(&sock, &Command::Toggle).unwrap();
        assert!(matches!(reply, Reply::Ok { .. }));
        responder.join().unwrap();
        server.shutdown();
        assert!(!sock.exists());
    }
}
