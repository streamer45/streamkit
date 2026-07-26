// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Regression test for the cross-session pixel leak.
//!
//! Two concurrent Servo instances render solid-colour `data:` pages; no
//! frame from one instance may ever contain the other's pixels.  Before
//! the first-paint gate in `handle_render`, the surfman surface reused
//! across instances meant a freshly-opened capture could emit a previous,
//! unrelated page's pixels.
//!
//! This lives in its own integration binary because it boots the real
//! Servo engine (software-rendered via llvmpipe — no GPU/display needed)
//! and is far too heavy for the workspace `cargo test`.

use std::io::Write;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use servo_web::test_api::{send_work, NodeId, ServoConfig, ServoThreadResult, ServoWorkItem};

const DIM: u32 = 64;
/// Per-instance tick budget — generous so the `data:` pages reach first
/// paint even on a slow/loaded CI box.  Each tick advances the shared
/// event loop once.
const MAX_TICKS: usize = 400;

fn send(item: ServoWorkItem) {
    if let Err(e) = send_work(item) {
        panic!("send_work failed: {e}");
    }
}

fn register(url: &str) -> (NodeId, Receiver<ServoThreadResult>) {
    let node_id = uuid::Uuid::new_v4();
    let (tx, rx) = std::sync::mpsc::sync_channel(2);
    let config =
        ServoConfig { url: url.to_string(), width: DIM, height: DIM, ..ServoConfig::default() };
    send(ServoWorkItem::Register { node_id, config, result_tx: tx });
    match rx.recv() {
        Ok(ServoThreadResult::InitOk) => {},
        Ok(ServoThreadResult::InitErr(e)) => panic!("init failed: {e}"),
        Ok(_) => panic!("unexpected result during init"),
        Err(e) => panic!("init recv failed: {e}"),
    }
    (node_id, rx)
}

fn render(node_id: NodeId, rx: &Receiver<ServoThreadResult>) -> Vec<u8> {
    send(ServoWorkItem::Render { node_id });
    match rx.recv() {
        Ok(ServoThreadResult::Frame { rgba_data, .. }) => rgba_data,
        Ok(_) => panic!("expected frame result"),
        Err(e) => panic!("frame recv failed: {e}"),
    }
}

/// Count opaque pixels that are predominantly red and predominantly blue.
fn count_red_blue(frame: &[u8]) -> (usize, usize) {
    let (mut red, mut blue) = (0usize, 0usize);
    for px in frame.chunks_exact(4) {
        let (r, g, b, a) = (px[0], px[1], px[2], px[3]);
        if a < 128 {
            continue;
        }
        if r > 200 && g < 80 && b < 80 {
            red += 1;
        } else if b > 200 && r < 80 && g < 80 {
            blue += 1;
        }
    }
    (red, blue)
}

#[test]
fn no_cross_session_pixel_leak() {
    let red_url = "data:text/html,<style>html,body{margin:0;height:100%;background:red}</style>";
    let blue_url = "data:text/html,<style>html,body{margin:0;height:100%;background:blue}</style>";

    let (red_id, red_rx) = register(red_url);
    let (blue_id, blue_rx) = register(blue_url);

    let mut red_saw_own = false;
    let mut blue_saw_own = false;

    for _ in 0..MAX_TICKS {
        let rf = render(red_id, &red_rx);
        let bf = render(blue_id, &blue_rx);

        let (rr, rb) = count_red_blue(&rf);
        let (br, bb) = count_red_blue(&bf);

        assert_eq!(rb, 0, "red instance leaked blue pixels");
        assert_eq!(br, 0, "blue instance leaked red pixels");

        red_saw_own |= rr > 0;
        blue_saw_own |= bb > 0;
        if red_saw_own && blue_saw_own {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    send(ServoWorkItem::Unregister { node_id: red_id });
    send(ServoWorkItem::Unregister { node_id: blue_id });

    // Otherwise the no-leak assertions above would be vacuous.
    assert!(red_saw_own, "red instance never painted its own page");
    assert!(blue_saw_own, "blue instance never painted its own page");

    // Servo's embedded engine has no clean global-shutdown path: at normal
    // process exit, mozjs' C++ static destructors race the still-live
    // renderer thread and abort with a SIGSEGV, which would fail this
    // otherwise-passing test.  This is the only test in this binary, so
    // exiting here — after all assertions have held — is safe and bypasses
    // that teardown entirely.
    println!("no_cross_session_pixel_leak: PASSED");
    let _ = std::io::stdout().flush();
    // SAFETY: `_exit` is async-signal-safe and simply terminates the
    // process with status 0 without running atexit handlers or C++ static
    // destructors (the source of the teardown SIGSEGV above).
    unsafe { libc::_exit(0) };
}
