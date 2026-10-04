//! QMP — QEMU Machine Protocol — client.
//!
//! QMP is newline-delimited JSON over a stream socket, which QEMU exposes when
//! launched with `-qmp tcp:127.0.0.1:4444,server,nowait`. The conversation is
//! fixed and short:
//!
//! ```text
//! S: {"QMP":{"version":{...},"capabilities":[...]}}\n
//! C: {"execute":"qmp_capabilities"}\n
//! S: {"return":{}}\n
//! C: {"execute":"screendump","arguments":{"filename":"...","format":"png"}}\n
//! S: {"return":{}}\n
//! ```
//!
//! Two details drive the whole design here.
//!
//! **Serialisation is mandatory, not politeness.** QMP is a single request /
//! single response stream with no request ids. Two commands in flight at once
//! means the second reader consumes the first reader's reply, and the failure
//! looks like "screendump returned somebody else's empty object" rather than
//! an error. Continuum's input model is *push* — a client sends an event and
//! the server applies it immediately — so interleaving is the normal case, not
//! a rare one. [`QmpClient`] therefore keeps its socket behind a mutex and
//! holds it across write-then-read, which makes every `execute` a transaction.
//!
//! **Events are interleaved with replies.** QEMU emits `{"event":"RESUME",...}`
//! at arbitrary points, including while a command is outstanding. A reader that
//! does not skip them will parse an event as an empty reply and silently
//! succeed with no data, which is how a screenshot turns into a zero-byte
//! frame. `_read_reply` skips events explicitly.

use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::Mutex;

/// Default QMP port. QEMU itself has no default; every harness that talks QMP
/// picks 4444, so we do too.
pub const DEFAULT_QMP_PORT: u16 = 4444;

/// How long to wait for the greeting, for each command reply, and for a TCP
/// connect. Ten seconds is generous for a loopback socket to a VM that is
/// already running, and short enough that a dead VM does not wedge a GUI
/// session behind a frame loop.
pub const DEFAULT_QMP_TIMEOUT: Duration = Duration::from_secs(10);

/// Errors from a QMP exchange.
#[derive(Debug, thiserror::Error)]
pub enum QmpError {
    #[error("could not connect to QMP at {addr}: {source}")]
    Connect {
        addr: SocketAddr,
        #[source]
        source: std::io::Error,
    },

    #[error("QMP I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("timed out after {elapsed_ms}ms waiting for QMP {what}")]
    Timeout { what: &'static str, elapsed_ms: u64 },

    #[error("QMP greeting was not a QMP greeting: {0}")]
    BadGreeting(String),

    #[error("QMP closed the connection while {what}")]
    Closed { what: &'static str },

    #[error("QMP replied to {command} with an error: {desc}")]
    Command { command: String, desc: String },

    #[error("QMP sent a line that is not JSON: {line}")]
    BadJson { line: String },
}

/// Result alias for QMP operations.
pub type QmpResult<T> = Result<T, QmpError>;

/// The framed half of a QMP connection: everything after the greeting.
struct QmpSocket {
    reader: BufReader<tokio::net::tcp::OwnedReadHalf>,
    writer: tokio::net::tcp::OwnedWriteHalf,
    timeout: Duration,
}

impl QmpSocket {
    /// Write one command and read its reply, skipping any events that arrive
    /// in between.
    ///
    /// The caller must hold the `QmpClient` lock; this is not safe to call
    /// concurrently.
    async fn execute(
        &mut self,
        command: &str,
        arguments: Option<serde_json::Value>,
    ) -> QmpResult<serde_json::Value> {
        let mut message = serde_json::Map::new();
        message.insert("execute".into(), serde_json::Value::String(command.into()));
        if let Some(args) = arguments {
            message.insert("arguments".into(), args);
        }
        // to_string escapes Windows backslashes, so a temp path such as
        // C:\Users\...\frame.png survives the trip intact. Hand-rolled string
        // concatenation would produce invalid JSON here.
        let payload = serde_json::to_string(&serde_json::Value::Object(message))
            .map_err(|e| QmpError::BadJson { line: e.to_string() })?;

        self.writer
            .write_all(payload.as_bytes())
            .await
            .map_err(QmpError::Io)?;
        self.writer.write_all(b"\n").await.map_err(QmpError::Io)?;
        self.writer.flush().await.map_err(QmpError::Io)?;

        self.read_reply(command).await
    }

    async fn read_reply(&mut self, command: &str) -> QmpResult<serde_json::Value> {
        loop {
            let line = self.read_line("a command reply").await?;
            let value: serde_json::Value = match serde_json::from_str(line.trim()) {
                Ok(v) => v,
                Err(_) => {
                    return Err(QmpError::BadJson {
                        line: line.chars().take(200).collect(),
                    })
                }
            };

            // Async event: not our answer, keep reading.
            if value.get("event").is_some() {
                tracing::trace!(event = ?value.get("event"), "QMP async event while awaiting reply");
                continue;
            }

            if let Some(error) = value.get("error") {
                let desc = error
                    .get("desc")
                    .and_then(|d| d.as_str())
                    .unwrap_or("no description")
                    .to_string();
                return Err(QmpError::Command {
                    command: command.to_string(),
                    desc,
                });
            }

            if let Some(ret) = value.get("return") {
                return Ok(ret.clone());
            }

            // A well-formed object with neither `return` nor `error` nor
            // `event` is not a reply we can use. Reporting it beats returning
            // an empty success.
            return Err(QmpError::BadJson {
                line: line.chars().take(200).collect(),
            });
        }
    }

    /// Read one newline-terminated line.
    ///
    /// `read_line` rather than `read_until` on a byte slice: QMP is line
    /// framed and a line can exceed any small buffer, and silently splitting a
    /// JSON object produces a parse error that blames the wrong thing.
    async fn read_line(&mut self, what: &'static str) -> QmpResult<String> {
        let mut line = String::new();
        let read = tokio::time::timeout(self.timeout, self.reader.read_line(&mut line)).await;
        match read {
            Err(_) => Err(QmpError::Timeout {
                what,
                elapsed_ms: self.timeout.as_millis() as u64,
            }),
            Ok(Err(e)) => Err(QmpError::Io(e)),
            // Zero bytes before a newline is a clean close, not an empty line.
            Ok(Ok(0)) => Err(QmpError::Closed { what }),
            Ok(Ok(_)) => Ok(line),
        }
    }
}

/// A connected QMP session.
///
/// Cheap to share: wrap in `Arc` and every concurrent caller is serialised at
/// the socket, which is exactly the invariant QMP needs.
pub struct QmpClient {
    socket: Mutex<QmpSocket>,
    addr: SocketAddr,
    timeout: Duration,
}

impl std::fmt::Debug for QmpClient {
    /// Hand-written: the halves of a TCP stream are not `Debug`, and a derived
    /// impl would only print the socket state anyway.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QmpClient")
            .field("addr", &self.addr)
            .field("timeout_ms", &self.timeout.as_millis())
            .finish_non_exhaustive()
    }
}

impl QmpClient {
    /// Connect to a QMP endpoint and complete the handshake.
    ///
    /// The handshake is not optional. QEMU refuses every command until it has
    /// received `qmp_capabilities`; it answers a premature command with
    /// `{"error":{"class":"GenericError","desc":"Expecting capabilities
    /// negotiation"}}`. Skipping it produces an error that reads like a
    /// protocol version mismatch.
    pub async fn connect(addr: SocketAddr) -> QmpResult<Self> {
        Self::connect_with_timeout(addr, DEFAULT_QMP_TIMEOUT).await
    }

    pub async fn connect_with_timeout(addr: SocketAddr, timeout: Duration) -> QmpResult<Self> {
        let stream = tokio::time::timeout(timeout, TcpStream::connect(addr))
            .await
            .map_err(|_| QmpError::Timeout {
                what: "connect",
                elapsed_ms: timeout.as_millis() as u64,
            })?
            .map_err(|source| QmpError::Connect { addr, source })?;

        // Nagle would coalesce a keystroke with whatever else is queued,
        // defeating the per-key pacing in `input_qmp`.
        let _ = stream.set_nodelay(true);

        let (read_half, write_half) = stream.into_split();
        let mut socket = QmpSocket {
            reader: BufReader::new(read_half),
            writer: write_half,
            timeout,
        };

        let greeting = socket.read_line("the QMP greeting").await?;
        let parsed: serde_json::Value = serde_json::from_str(greeting.trim()).map_err(|_| {
            QmpError::BadGreeting(greeting.chars().take(200).collect())
        })?;
        if parsed.get("QMP").is_none() {
            return Err(QmpError::BadGreeting(greeting.chars().take(200).collect()));
        }

        let client = Self {
            socket: Mutex::new(socket),
            addr,
            timeout,
        };
        client.execute("qmp_capabilities", None).await?;
        tracing::info!(%addr, "QMP session established");
        Ok(client)
    }

    /// Issue one QMP command and return its `return` payload.
    ///
    /// Serialised against every other caller on this connection.
    pub async fn execute(
        &self,
        command: &str,
        arguments: Option<serde_json::Value>,
    ) -> QmpResult<serde_json::Value> {
        let mut socket = self.socket.lock().await;
        let result = socket.execute(command, arguments).await;
        drop(socket);
        result
    }

    /// Run a human-monitor command, i.e. QMP's passthrough to the HMP prompt.
    ///
    /// This is how keys are injected: HMP's `sendkey` accepts key *names*
    /// directly, which is why the key vocabulary in `input_qmp` is a list of
    /// names rather than a keycode table.
    pub async fn hmp(&self, command_line: &str) -> QmpResult<serde_json::Value> {
        self.execute(
            "human-monitor-command",
            Some(serde_json::json!({ "command-line": command_line })),
        )
        .await
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;
    use tokio::io::AsyncReadExt;
    use tokio::net::TcpListener;

    const GREETING: &str = r#"{"QMP":{"version":{"package":"8.1.0"},"capabilities":[]}}"#;

    /// A fake QEMU: send the greeting, then answer each request line by looking up
    /// the first rule whose needle appears in that line.
    ///
    /// Keyed on the request rather than on a call counter: TCP coalesces, so
    /// two requests can arrive in one read and a counter-based script hands the
    /// second reply to the first caller.
    ///
    /// Each rule yields a list of lines, so a rule can emit async events before
    /// its reply.
    async fn spawn_fake_qmp(rules: Vec<(&'static str, Vec<String>)>) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = match listener.accept().await {
                Ok(pair) => pair,
                Err(_) => return,
            };
            socket.write_all(format!("{GREETING}\n").as_bytes()).await.unwrap();

            let mut buf = vec![0u8; 8192];
            loop {
                let n = match socket.read(&mut buf).await {
                    Ok(0) | Err(_) => return,
                    Ok(n) => n,
                };
                let text = String::from_utf8_lossy(&buf[..n]).to_string();
                for line in text.lines() {
                    if line.trim().is_empty() {
                        continue;
                    }
                    let replies = rules
                        .iter()
                        .find(|(needle, _)| line.contains(needle))
                        .map(|(_, replies)| replies.clone())
                        .unwrap_or_else(|| vec![r#"{"return":{}}"#.to_string()]);
                    for reply in replies {
                        if socket
                            .write_all(format!("{reply}\n").as_bytes())
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                }
            }
        });
        addr
    }

    #[tokio::test]
    async fn test_handshake_then_command() {
        let addr = spawn_fake_qmp(vec![("screendump", vec![r#"{"return":{}}"#.into()])]).await;

        let client = QmpClient::connect(addr).await.unwrap();
        client
            .execute(
                "screendump",
                Some(serde_json::json!({"filename": "C:\\t\\f.png", "format": "png"})),
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_async_events_are_skipped_not_returned() {
        let addr = spawn_fake_qmp(vec![(
            "query-status",
            vec![
                r#"{"event":"RESUME","data":{}}"#.into(),
                r#"{"event":"STOP","data":{}}"#.into(),
                r#"{"return":{"size":42}}"#.into(),
            ],
        )])
        .await;

        let client = QmpClient::connect(addr).await.unwrap();
        let ret = client.execute("query-status", None).await.unwrap();
        assert_eq!(
            ret.get("size").and_then(|v| v.as_u64()),
            Some(42),
            "events must be skipped, not returned as an empty success"
        );
    }

    #[tokio::test]
    async fn test_command_error_is_surfaced() {
        let addr = spawn_fake_qmp(vec![(
            "input-send-event",
            vec![
                r#"{"error":{"class":"GenericError","desc":"Device 'usb-tablet' not found"}}"#
                    .into(),
            ],
        )])
        .await;

        let client = QmpClient::connect(addr).await.unwrap();
        let err = client.execute("input-send-event", None).await.unwrap_err();
        match err {
            QmpError::Command { desc, .. } => {
                assert!(desc.contains("usb-tablet"), "unexpected desc: {desc}");
            }
            other => panic!("expected Command error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_hmp_builds_the_right_envelope() {
        let addr = spawn_fake_qmp(vec![(
            "human-monitor-command",
            vec![r#"{"return":"\r\n"}"#.into()],
        )])
        .await;

        let client = QmpClient::connect(addr).await.unwrap();
        let ret = client.hmp("sendkey shift-a").await.unwrap();
        assert_eq!(ret.as_str(), Some("\r\n"));
    }

    #[tokio::test]
    async fn test_connection_refused_is_an_error_not_a_panic() {
        // Port 1 on loopback: nothing listens there.
        let addr: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let err = QmpClient::connect_with_timeout(addr, Duration::from_millis(500))
            .await
            .unwrap_err();
        assert!(
            matches!(err, QmpError::Connect { .. } | QmpError::Timeout { .. }),
            "got {err:?}"
        );
    }

    #[tokio::test]
    async fn test_non_qmp_greeting_is_rejected() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            socket.write_all(b"hello there\n").await.unwrap();
            std::future::pending::<()>().await;
        });

        let err = QmpClient::connect(addr).await.unwrap_err();
        assert!(
            matches!(err, QmpError::BadGreeting(_)),
            "got {err:?}"
        );
    }

    #[test]
    fn test_default_port_matches_common_harnesses() {
        assert_eq!(DEFAULT_QMP_PORT, 4444);
    }
}