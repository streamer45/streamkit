// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use bytes::Bytes;
use std::collections::VecDeque;
use std::io::Read;
use tokio::sync::mpsc;

/// A bounded, zero-copy Read implementation for streaming data.
///
/// This reader uses a `VecDeque<Bytes>` and drops chunks after they're consumed.
/// Memory usage is bounded when used with a bounded tokio channel.
///
/// Key design principles:
/// - **Bounded memory**: Upstream is backpressured via bounded channel capacity
/// - **Zero-copy receive**: Accepts `Bytes` directly (no Vec<u8> conversion)
/// - **Blocking recv**: Blocks waiting for data, allowing upstream to pace delivery
///
/// # Safety Invariant
///
/// **IMPORTANT**: This reader uses `blocking_recv()` which will block the current thread.
/// It MUST only be used inside `tokio::task::spawn_blocking` contexts. Using it on a Tokio
/// worker thread will stall the runtime and cause latency spikes for unrelated tasks.
///
/// Correct usage:
/// ```ignore
/// let rx = mpsc::channel(32);
/// tokio::task::spawn_blocking(move || {
///     let reader = StreamingReader::new(rx);
///     // ... use reader.read() safely here ...
/// });
/// ```
pub struct StreamingReader {
    /// Queue of pending chunks - consumed chunks are dropped
    chunks: VecDeque<Bytes>,
    /// Current read position within the front chunk
    chunk_offset: usize,
    /// Channel receiver for incoming data
    rx: mpsc::Receiver<Bytes>,
    eof: bool,
}

impl StreamingReader {
    /// Create a new streaming reader from a bounded tokio receiver.
    pub const fn new(rx: mpsc::Receiver<Bytes>) -> Self {
        Self { chunks: VecDeque::new(), chunk_offset: 0, rx, eof: false }
    }
}

impl Read for StreamingReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        loop {
            // Try to read from current chunk
            if let Some(front) = self.chunks.front() {
                let available = front.len() - self.chunk_offset;
                if available > 0 {
                    let to_read = available.min(buf.len());
                    buf[..to_read]
                        .copy_from_slice(&front[self.chunk_offset..self.chunk_offset + to_read]);
                    self.chunk_offset += to_read;

                    // If we've consumed the entire chunk, drop it
                    if self.chunk_offset >= front.len() {
                        self.chunks.pop_front();
                        self.chunk_offset = 0;
                        tracing::trace!(
                            "StreamingReader: Dropped consumed chunk, {} chunks remaining",
                            self.chunks.len()
                        );
                    }

                    return Ok(to_read);
                }
                // Empty chunk - drop it and try next
                self.chunks.pop_front();
                self.chunk_offset = 0;
                continue;
            }

            if self.eof {
                return Ok(0);
            }

            match self.rx.blocking_recv() {
                Some(chunk) if !chunk.is_empty() => self.chunks.push_back(chunk),
                Some(_) => {},
                None => self.eof = true,
            }
        }
    }
}
