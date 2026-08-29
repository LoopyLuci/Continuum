use continuum_transport::ClientConfig;
use std::sync::mpsc;

pub use continuum_transport::client::ControlCommand;

#[allow(dead_code)]
pub struct NetworkWorker {
    pub event_rx: mpsc::Receiver<WorkerEvent>,
    pub command_tx: mpsc::Sender<ControlCommand>,
    _thread: std::thread::JoinHandle<()>,
}

#[allow(dead_code)]
pub enum WorkerEvent {
    Frame(Vec<u8>),
    Status(String),
    Clipboard(Vec<u8>),
    PairingResult {
        accepted: bool,
        #[allow(dead_code)]
        message: String,
        sas_words: Vec<String>,
    },
    Error(String),
}

#[allow(dead_code)]
impl NetworkWorker {
    pub fn spawn(addr: std::net::SocketAddr, password: String) -> Self {
        let (event_tx, event_rx) = mpsc::channel();
        let (cmd_tx, cmd_rx) = mpsc::channel();

        let thread = std::thread::Builder::new()
            .name("continuum-net".to_string())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .worker_threads(2)
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        tracing::error!(error = %e, "Failed to create network runtime");
                        return;
                    }
                };
                rt.block_on(async move {
                    let (tokio_event_tx, mut tokio_rx) = tokio::sync::mpsc::channel::<
                        continuum_transport::client::ConnectionEvent,
                    >(256);

                    // Forward events to std mpsc
                    let tx = event_tx.clone();
                    tokio::spawn(async move {
                        while let Some(evt) = tokio_rx.recv().await {
                            let out = match evt {
                                continuum_transport::client::ConnectionEvent::Frame {
                                    data,
                                    ..
                                } => WorkerEvent::Frame(data),
                                continuum_transport::client::ConnectionEvent::Status(s) => {
                                    WorkerEvent::Status(s.to_string())
                                }
                                continuum_transport::client::ConnectionEvent::Clipboard(d) => {
                                    WorkerEvent::Clipboard(d.data)
                                }
                                continuum_transport::client::ConnectionEvent::Error(e) => {
                                    WorkerEvent::Error(e)
                                }
                                continuum_transport::client::ConnectionEvent::PairingResult(r) => {
                                    let evt = WorkerEvent::PairingResult {
                                        accepted: r.accepted,
                                        message: r.message.clone(),
                                        sas_words: r.sas_words.clone(),
                                    };
                                    if tx.send(evt).is_err() {
                                        break;
                                    }
                                    if r.accepted {
                                        let _ = tx.send(WorkerEvent::Status("Streaming".into()));
                                    } else {
                                        let _ = tx.send(WorkerEvent::Error(format!(
                                            "Pairing failed: {}",
                                            r.message
                                        )));
                                    }
                                    continue;
                                }
                                continuum_transport::client::ConnectionEvent::Metrics(_) => {
                                    continue
                                }
                            };
                            if tx.send(out).is_err() {
                                break;
                            }
                        }
                    });

                    // Forward commands from std mpsc to tokio
                    let (tokio_cmd_tx, tokio_cmd_rx) =
                        tokio::sync::mpsc::channel::<ControlCommand>(32);
                    tokio::spawn(async move {
                        while let Ok(cmd) = cmd_rx.recv() {
                            let _ = tokio_cmd_tx.send(cmd).await;
                        }
                    });

                    let config = ClientConfig {
                        server_addr: addr,
                        pairing_code: password,
                        auto_reconnect: true,
                        ..Default::default()
                    };
                    let _ = continuum_transport::client::maintain_connection(
                        &config,
                        tokio_event_tx,
                        tokio_cmd_rx,
                    )
                    .await;
                });
            })
            .expect("Failed to spawn network thread — check system resources");

        Self {
            event_rx,
            command_tx: cmd_tx,
            _thread: thread,
        }
    }
}
