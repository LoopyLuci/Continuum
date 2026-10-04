use anyhow::Result;
use clap::Parser;
use continuum_transport::{ServerArgs, ServerConfig};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    let args = ServerArgs::parse();
    let config = ServerConfig::load(&args)?;

    if config.insecure {
        eprintln!("WARNING: Running with --insecure flag. E2E encryption is DISABLED.");
        eprintln!("This is intended for local testing ONLY. Do not use in production.");
        eprintln!("All media frames will be transmitted in PLAINTEXT.");
        eprintln!();
    }

    let log_level = if args.verbose {
        "debug"
    } else {
        &config.log_level
    };
    init_logging(log_level, &config.log_format);

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        "Starting Continuum server"
    );
    tracing::info!(addr = %config.listen_addr, "Configuration loaded");
    tracing::info!(pairing_code = %config.pairing_code, quality = config.quality, fps = config.target_fps, "Server parameters");

    // The QMP sidecar is a separate, loopback-only listener for the local GUI.
    // It is optional: an unusable configuration (a bad port, a malformed VM
    // list) must not stop the QUIC server from coming up.
let sidecar = match continuum_transport::ws_sidecar::WsSidecar::from_env() {
        Ok(sidecar) => {
            // The sidecar injects keystrokes and pointer events into real
            // guests, so clients must present this token before they get
            // anything. Log it once: an operator cannot use the feature
            // otherwise, and there is no interactive prompt here.
            tracing::info!(
                "QMP sidecar on ws://{}/ws/stream - auth token: {}",
                sidecar.addr(),
                sidecar.token(),
            );
            Some(sidecar)
        }
        Err(e) => {
            tracing::warn!(error = %e, "QMP sidecar disabled - GUI screen sharing unavailable");
            None
        }
    };

    let shutdown = shutdown_signal();

    tokio::select! {
        result = continuum_transport::run_server(config) => {
            if let Err(err) = result {
                tracing::error!(error = %err, "Server error");
                return Err(err);
            }
        }
        result = async {
            match sidecar {
                Some(sidecar) => sidecar.serve().await,
                // Nothing to serve; park so the select! still has a branch.
                None => std::future::pending().await,
            }
        } => {
            if let Err(err) = result {
                // A sidecar failure is not fatal to remote desktop: log it and
                // let the QUIC server carry on.
                tracing::error!(error = %err, "QMP sidecar error");
            }
        }
        _ = shutdown => {
            tracing::info!("Shutdown signal received, stopping server");
        }
    }

    tracing::info!("Server stopped");
    Ok(())
}

fn init_logging(level: &str, format: &str) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level));

    match format {
        "json" => {
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .json()
                .init();
        }
        _ => {
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_target(true)
                .with_thread_ids(true)
                .with_file(true)
                .with_line_number(true)
                .init();
        }
    }
}

async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();

    #[cfg(unix)]
    {
        let mut sigterm =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).unwrap();
        tokio::select! {
            _ = ctrl_c => {}
            _ = sigterm.recv() => {}
        }
    }

    #[cfg(not(unix))]
    {
        ctrl_c.await.ok();
    }
}
