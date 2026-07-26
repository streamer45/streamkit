// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Regression test: animated pages must produce changing frames.
//!
//! A `data:` page drives a `requestAnimationFrame` loop that clears a
//! WebGL2 canvas with a colour that changes every frame.  Successive
//! rendered frames must differ; a frozen animation (e.g. WebGL2 disabled,
//! so `getContext('webgl2')` returns null and the page's render loop
//! never starts) fails the test.
//!
//! This lives in its own integration binary because it boots the real
//! Servo engine (software-rendered via llvmpipe — no GPU/display needed)
//! and is far too heavy for the workspace `cargo test`.

use std::ffi::c_char;
use std::io::Write;
use std::os::raw::c_void;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use servo_web::test_api::{
    send_work, CLogLevel, Logger, NodeId, ServoConfig, ServoThreadResult, ServoWorkItem,
};

const extern "C" fn noop_log(
    _level: CLogLevel,
    _target: *const c_char,
    _message: *const c_char,
    _user_data: *mut c_void,
) {
}

const DIM: u32 = 64;
/// Generous tick budget so the page reaches first paint and animates even
/// on a slow/loaded CI box.
const MAX_TICKS: usize = 600;
/// Frames that differ from their predecessor before declaring success.
const REQUIRED_CHANGES: usize = 10;

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
    let logger = Logger::new(noop_log, std::ptr::null_mut(), "servo-test");
    send(ServoWorkItem::Register { node_id, config, result_tx: tx, logger });
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

#[test]
fn animated_webgl2_page_frames_change() {
    // No webgl1 fallback: the test must fail if WebGL2 is unavailable,
    // since real pages (e.g. webgl2fundamentals.org's background) bail out
    // of their animation loop entirely when getContext('webgl2') is null.
    let url = "data:text/html,<body style='margin:0'>\
        <canvas id=c width=64 height=64></canvas>\
        <script>\
        const gl=document.getElementById('c').getContext('webgl2');\
        if(gl){let n=0;function f(){n=(n+0.03)%1;\
        gl.clearColor(n,1-n,0.5,1);gl.clear(gl.COLOR_BUFFER_BIT);\
        requestAnimationFrame(f)}requestAnimationFrame(f)}\
        </script></body>";

    let (id, rx) = register(url);

    let mut last: Option<Vec<u8>> = None;
    let mut changes = 0usize;
    let mut painted_frames = 0usize;
    for _ in 0..MAX_TICKS {
        let frame = render(id, &rx);
        // Pre-paint frames are fully transparent; only compare painted ones.
        if frame.chunks_exact(4).any(|px| px[3] >= 128) {
            painted_frames += 1;
            if last.as_deref().is_some_and(|prev| prev != frame) {
                changes += 1;
            }
            last = Some(frame);
        }
        if changes >= REQUIRED_CHANGES {
            break;
        }
        std::thread::sleep(Duration::from_millis(33));
    }

    send(ServoWorkItem::Unregister { node_id: id });

    println!("painted_frames = {painted_frames}, changed_frames = {changes}");
    assert!(painted_frames > 0, "page never painted");
    assert!(
        changes >= REQUIRED_CHANGES,
        "animation is frozen: only {changes} of {painted_frames} painted frames changed"
    );

    // Servo's embedded engine has no clean global-shutdown path: at normal
    // process exit, mozjs' C++ static destructors race the still-live
    // renderer thread and abort with a SIGSEGV, which would fail this
    // otherwise-passing test.  This is the only test in this binary, so
    // exiting here — after all assertions have held — is safe and bypasses
    // that teardown entirely.
    println!("animated_webgl2_page_frames_change: PASSED");
    let _ = std::io::stdout().flush();
    // SAFETY: `_exit` is async-signal-safe and simply terminates the
    // process with status 0 without running atexit handlers or C++ static
    // destructors (the source of the teardown SIGSEGV above).
    unsafe { libc::_exit(0) };
}
