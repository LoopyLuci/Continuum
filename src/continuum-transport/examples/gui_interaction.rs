// Drive a real graphical application with Continuum's own input path.
//
//   cargo run -p continuum-transport --example gui_interaction
//
// # Why this exists
//
// Everything else in this crate's test suite proves bytes moved correctly.
// Nothing proved that a keystroke typed by `QmpInputInjector` lands in a real
// window inside a real desktop and does something observable. A guest sitting
// at a login prompt accepts every keystroke and discards them all, which looks
// exactly like a working input path from the outside.
//
// So this launches a terminal, types a command that leaves a trace *outside*
// the GUI, and then checks for that trace. The window appearing is not the
// test; the file existing is. Anything that silently swallows input -- a window
// that never took focus, a keymap that drops the sequence, a compositor that
// ignores synthetic events -- fails it.
//
// The trace is checked over SSH, deliberately: a different transport, so the
// check cannot be satisfied by the same path under test.

use std::sync::Arc;
use std::time::Duration;

use continuum_transport::capture_source::CaptureSource;
use continuum_transport::input_qmp::QmpInputInjector;

fn qmp() -> std::net::SocketAddr {
    std::net::SocketAddr::from(([127, 0, 0, 1], 4444))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let source = match CaptureSource::connect("omarchy", qmp(), 85, 1280, 800).await {
        Ok((source, reason)) => {
            if let Some(reason) = reason {
                eprintln!("note: video fell back to QMP ({reason})");
            }
            source
        }
        Err(e) => {
            eprintln!("SKIPPED: no guest on QMP {}: {e}", qmp());
            return Ok(());
        }
    };

    let injector = Arc::new(
        QmpInputInjector::new("omarchy", source.client(), None).with_guest_size(1280, 800),
    );

    // Launch a terminal in the running desktop, then check the compositor
    // agrees a window exists. Launching blind would race: typing before the
    // window has focus types into whatever had it, which is usually nothing.
    //
    // The app is started directly against the compositor's Wayland socket
    // rather than through `hyprctl dispatch exec`. That call is not usable as
    // a stable interface here: Hyprland 0.56 moved hyprctl onto a Lua
    // front end, and `dispatch exec foot` now fails to parse while still
    // launching the app -- so a check built on its exit status would be
    // asserting something unrelated to whether the app started.
    let marker = format!("/tmp/gui-proof-{}", std::process::id());
    println!("launching foot and waiting for it to map");
    let launched = ssh(&format!(
        "pkill -x foot 2>/dev/null; rm -f {marker}; \
         XDG_RUNTIME_DIR=/run/user/1000 WAYLAND_DISPLAY=wayland-1 \
         setsid foot >/dev/null 2>&1 < /dev/null & sleep 1; echo launched"
    ))?;
    println!("{launched}");

    let mut mapped = false;
    for _ in 0..30 {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let clients = ssh(
            "XDG_RUNTIME_DIR=/run/user/1000 \
             HYPRLAND_INSTANCE_SIGNATURE=$(ls /run/user/1000/hypr/ | head -1) \
             hyprctl clients 2>&1",
        )
        .unwrap_or_default();
        if clients.contains("foot") && clients.contains("mapped: 1") {
            mapped = true;
            break;
        }
    }
    if !mapped {
        eprintln!("FAILED: no foot window appeared in the compositor");
        return Err("foot did not map".into());
    }
    println!("OK: foot is mapped; typing through Continuum's input injector");

    // Type a command whose effect is visible from outside the GUI.
    injector.type_text(&format!("touch {marker}")).await?;
    injector.send_key_name("ret").await?;

    // Give the shell a moment, then look for the trace.
    let mut found = false;
    for _ in 0..20 {
        tokio::time::sleep(Duration::from_millis(500)).await;
        if ssh(&format!("test -f {marker} && echo PRESENT")).is_ok_and(|o| o.contains("PRESENT")) {
            found = true;
            break;
        }
    }

    ssh(&format!("rm -f {marker}"))?;

    if found {
        println!("OK: a keystroke typed over QMP executed inside a real desktop window");
        Ok(())
    } else {
        Err("keystrokes never reached the application".into())
    }
}

/// Run a command over SSH in the guest.
///
/// Returns an error when the command fails, so a failure here is never
/// mistaken for an empty result.
fn ssh(command: &str) -> Result<String, Box<dyn std::error::Error>> {
    let key = format!("{}/.ssh/id_ed25519", std::env::var("USERPROFILE").unwrap_or_default());
    let output = std::process::Command::new("ssh")
        .args([
            "-o", "BatchMode=yes",
            "-o", "StrictHostKeyChecking=no",
            "-o", "ConnectTimeout=15",
            "-p", "2222",
            "-i", &key,
            "OmarchyVM@127.0.0.1",
            command,
        ])
        .output()?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !output.status.success() {
        return Err(format!(
            "ssh failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    Ok(stdout)
}