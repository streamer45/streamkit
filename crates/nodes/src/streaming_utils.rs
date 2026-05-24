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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // Test assertions use unwrap/expect to fail loudly.
mod tests {
    use super::*;
    use std::io::Read;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn read_single_chunk() {
        let (tx, rx) = mpsc::channel(10);
        tx.send(Bytes::from_static(b"hello")).await.unwrap();
        drop(tx);

        let result = tokio::task::spawn_blocking(move || {
            let mut reader = StreamingReader::new(rx);
            let mut buf = vec![0u8; 1024];
            let n = reader.read(&mut buf).unwrap();
            buf.truncate(n);
            buf
        })
        .await
        .unwrap();

        assert_eq!(result, b"hello");
    }

    #[tokio::test]
    async fn read_multiple_chunks() {
        let (tx, rx) = mpsc::channel(10);
        tx.send(Bytes::from_static(b"hello")).await.unwrap();
        tx.send(Bytes::from_static(b" world")).await.unwrap();
        drop(tx);

        let result = tokio::task::spawn_blocking(move || {
            let mut reader = StreamingReader::new(rx);
            let mut all = Vec::new();
            let mut buf = [0u8; 1024];
            loop {
                let n = reader.read(&mut buf).unwrap();
                if n == 0 {
                    break;
                }
                all.extend_from_slice(&buf[..n]);
            }
            all
        })
        .await
        .unwrap();

        assert_eq!(result, b"hello world");
    }

    #[tokio::test]
    async fn read_partial_small_buffer() {
        let (tx, rx) = mpsc::channel(10);
        tx.send(Bytes::from_static(b"abcdefgh")).await.unwrap();
        drop(tx);

        let result = tokio::task::spawn_blocking(move || {
            let mut reader = StreamingReader::new(rx);
            let mut buf = [0u8; 3];

            let n1 = reader.read(&mut buf).unwrap();
            let part1 = buf[..n1].to_vec();

            let n2 = reader.read(&mut buf).unwrap();
            let part2 = buf[..n2].to_vec();

            let n3 = reader.read(&mut buf).unwrap();
            let part3 = buf[..n3].to_vec();

            (part1, part2, part3)
        })
        .await
        .unwrap();

        assert_eq!(result.0, b"abc");
        assert_eq!(result.1, b"def");
        assert_eq!(result.2, b"gh");
    }

    #[tokio::test]
    async fn read_skips_empty_chunks() {
        let (tx, rx) = mpsc::channel(10);
        tx.send(Bytes::new()).await.unwrap();
        tx.send(Bytes::from_static(b"data")).await.unwrap();
        tx.send(Bytes::new()).await.unwrap();
        drop(tx);

        let result = tokio::task::spawn_blocking(move || {
            let mut reader = StreamingReader::new(rx);
            let mut buf = vec![0u8; 1024];
            let n = reader.read(&mut buf).unwrap();
            buf.truncate(n);
            buf
        })
        .await
        .unwrap();

        assert_eq!(result, b"data");
    }

    #[tokio::test]
    async fn read_channel_close_returns_eof() {
        let (tx, rx) = mpsc::channel::<Bytes>(10);
        drop(tx);

        let result = tokio::task::spawn_blocking(move || {
            let mut reader = StreamingReader::new(rx);
            let mut buf = [0u8; 64];
            reader.read(&mut buf).unwrap()
        })
        .await
        .unwrap();

        assert_eq!(result, 0);
    }

    #[tokio::test]
    async fn read_after_eof_returns_zero() {
        let (tx, rx) = mpsc::channel(10);
        tx.send(Bytes::from_static(b"x")).await.unwrap();
        drop(tx);

        let result = tokio::task::spawn_blocking(move || {
            let mut reader = StreamingReader::new(rx);
            let mut buf = [0u8; 64];

            let n1 = reader.read(&mut buf).unwrap();
            assert_eq!(n1, 1);

            let n2 = reader.read(&mut buf).unwrap();
            assert_eq!(n2, 0);

            reader.read(&mut buf).unwrap()
        })
        .await
        .unwrap();

        assert_eq!(result, 0);
    }
}
