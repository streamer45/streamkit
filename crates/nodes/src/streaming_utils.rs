// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use bytes::Bytes;
use std::collections::VecDeque;
use std::io::Read;
use tokio::sync::mpsc;

/// Bounded, zero-copy `Read` over a channel of `Bytes` chunks.
///
/// Uses `blocking_recv()` — **must** run inside `spawn_blocking`.
pub struct StreamingReader {
    chunks: VecDeque<Bytes>,
    chunk_offset: usize,
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
            if let Some(front) = self.chunks.front() {
                let available = front.len() - self.chunk_offset;
                if available > 0 {
                    let to_read = available.min(buf.len());
                    buf[..to_read]
                        .copy_from_slice(&front[self.chunk_offset..self.chunk_offset + to_read]);
                    self.chunk_offset += to_read;

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
