// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Shared File-mode temp-file plumbing for the container muxers.
//!
//! File-mode container muxers write to an on-disk temp file so they can seek and
//! back-patch without holding the whole output in memory. At finalization the
//! temp file is streamed downstream in bounded chunks (one `Packet::Binary` per
//! chunk) so peak memory stays bounded regardless of output size.
//!
//! This module is the single home for that machinery so the MP4 and WebM muxers
//! don't each carry a copy: the seekable [`FileBackedBuffer`] writer, the bounded
//! [`ChunkedFileReader`], and the [`emit_file_in_chunks`] finalize helper.

use bytes::Bytes;
use std::borrow::Cow;
use std::io::{BufWriter, ErrorKind, Read as _, Seek, SeekFrom, Write};
use std::num::NonZeroUsize;
use streamkit_core::stats::NodeStatsTracker;
use streamkit_core::types::Packet;
use streamkit_core::{state_helpers, NodeContext, StreamKitError};

/// Default chunk size used when streaming a finalized File-mode temp file
/// downstream, applied when a muxer config leaves the size unset.
pub const FILE_MODE_CHUNK_SIZE: usize = 256 * 1024;

/// Resolve a configured File-mode finalize chunk size, falling back to
/// [`FILE_MODE_CHUNK_SIZE`] when unset.  `NonZeroUsize` already rejects zero at
/// deserialization, so the result is always a usable (non-zero) size.
pub fn resolve_finalize_chunk_size(configured: Option<NonZeroUsize>) -> usize {
    configured.map_or(FILE_MODE_CHUNK_SIZE, NonZeroUsize::get)
}

/// A file-backed buffer for container **File** mode muxing.
///
/// All writes go to an anonymous temporary file on disk so the muxer can
/// seek/back-patch without accumulating the entire output in memory.  The temp
/// file is deleted automatically when the buffer is dropped.
pub struct FileBackedBuffer {
    inner: BufWriter<std::fs::File>,
}

impl FileBackedBuffer {
    /// Create a new file-backed buffer using an anonymous temp file.
    pub fn new() -> std::io::Result<Self> {
        let file = tempfile::tempfile()?;
        Ok(Self { inner: BufWriter::new(file) })
    }

    /// Flush buffered writes and return the inner temp file for chunked read-back.
    pub fn finalized_file(&mut self) -> std::io::Result<&mut std::fs::File> {
        self.inner.flush()?;
        Ok(self.inner.get_mut())
    }

    /// Current write position in the file.
    pub fn position(&mut self) -> std::io::Result<u64> {
        self.inner.flush()?;
        self.inner.get_mut().stream_position()
    }
}

impl Write for FileBackedBuffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

impl Seek for FileBackedBuffer {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(pos)
    }
}

/// Reads a finalized temp file back in bounded chunks.
///
/// Each [`ChunkedFileReader::next_chunk`] call returns at most `chunk_size`
/// bytes, so the caller can emit one packet per chunk without ever allocating a
/// buffer the size of the whole file.
///
/// The total length is captured once at construction: a reader is only valid
/// over a *finalized* temp file that is no longer being written or truncated.
pub struct ChunkedFileReader<'a> {
    file: &'a mut std::fs::File,
    remaining: u64,
    chunk_size: usize,
}

impl<'a> ChunkedFileReader<'a> {
    /// Seek `file` back to the start and prepare to read it in chunks.
    ///
    /// Returns `Ok(None)` when the file is empty and `Err` when `chunk_size`
    /// is zero (which would otherwise never make progress).
    pub fn new(file: &'a mut std::fs::File, chunk_size: usize) -> std::io::Result<Option<Self>> {
        if chunk_size == 0 {
            return Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                "chunk_size must be greater than zero",
            ));
        }
        let len = file.seek(SeekFrom::End(0))?;
        if len == 0 {
            return Ok(None);
        }
        file.seek(SeekFrom::Start(0))?;
        Ok(Some(Self { file, remaining: len, chunk_size }))
    }

    /// Read the next chunk, or `Ok(None)` once the whole file has been read.
    pub fn next_chunk(&mut self) -> std::io::Result<Option<Bytes>> {
        if self.remaining == 0 {
            return Ok(None);
        }
        let take = self.remaining.min(self.chunk_size as u64);
        let want = usize::try_from(take).map_err(std::io::Error::other)?;
        let mut buf = Vec::with_capacity(want);
        let read = (&mut *self.file).take(take).read_to_end(&mut buf)?;
        if read != want {
            return Err(std::io::Error::new(
                ErrorKind::UnexpectedEof,
                "temp file ended before the expected chunk was read back",
            ));
        }
        self.remaining -= take;
        Ok(Some(Bytes::from(buf)))
    }
}

/// Stream a finalized temp `file` downstream as bounded `Packet::Binary` chunks.
///
/// Reads `file` in `chunk_size`-byte chunks and emits one packet per chunk on
/// the `out` pin, so peak memory stays bounded by `chunk_size` regardless of
/// output size.  A final stats snapshot is force-sent once any data was read,
/// even if the downstream channel closed mid-send.  Read-back IO errors emit a
/// `Failed` state (so subscribers see a terminal state) before propagating.
pub async fn emit_file_in_chunks(
    context: &mut NodeContext,
    file: &mut std::fs::File,
    chunk_size: usize,
    content_type: Cow<'static, str>,
    stats_tracker: &mut NodeStatsTracker,
    node_name: &str,
) -> Result<(), StreamKitError> {
    let read_err = |context: &NodeContext, e: std::io::Error| {
        let msg = format!("Failed to read back container file: {e}");
        state_helpers::emit_failed(&context.state_tx, node_name, &msg);
        StreamKitError::Runtime(msg)
    };

    let Some(mut reader) =
        ChunkedFileReader::new(file, chunk_size).map_err(|e| read_err(context, e))?
    else {
        return Ok(());
    };

    loop {
        let chunk = match reader.next_chunk() {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            Err(e) => return Err(read_err(context, e)),
        };
        tracing::debug!("Sending finalized container chunk ({} bytes)", chunk.len());
        if context
            .output_sender
            .send(
                "out",
                Packet::Binary {
                    data: chunk,
                    content_type: Some(content_type.clone()),
                    metadata: None,
                },
            )
            .await
            .is_err()
        {
            tracing::debug!("Output channel closed during final send");
            break;
        }
        stats_tracker.sent();
    }
    stats_tracker.force_send();
    Ok(())
}

#[cfg(test)]
// Tests use unwrap to fail loudly on setup/read errors.
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn temp_file_with(bytes: &[u8]) -> std::fs::File {
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(bytes).unwrap();
        file
    }

    fn collect(mut file: std::fs::File, chunk_size: usize) -> Vec<Bytes> {
        let mut out = Vec::new();
        if let Some(mut reader) = ChunkedFileReader::new(&mut file, chunk_size).unwrap() {
            while let Some(chunk) = reader.next_chunk().unwrap() {
                out.push(chunk);
            }
        }
        out
    }

    #[test]
    fn empty_file_yields_no_reader() {
        let mut file = temp_file_with(&[]);
        assert!(ChunkedFileReader::new(&mut file, 64).unwrap().is_none());
    }

    #[test]
    fn streams_in_bounded_chunks_and_preserves_bytes() {
        let chunk_size = 64;
        let original: Vec<u8> = (0..600u32).map(|i| u8::try_from(i % 251).unwrap()).collect();

        let chunks = collect(temp_file_with(&original), chunk_size);

        assert!(chunks.len() > 1, "output larger than one chunk must emit multiple packets");
        for chunk in &chunks {
            assert!(chunk.len() <= chunk_size, "no chunk may exceed the chunk size");
        }
        let joined: Vec<u8> = chunks.iter().flat_map(|c| c.iter().copied()).collect();
        assert_eq!(joined, original, "concatenated chunks must be byte-identical to the input");
    }

    #[test]
    fn zero_chunk_size_is_rejected() {
        let mut file = temp_file_with(&[1, 2, 3]);
        assert!(ChunkedFileReader::new(&mut file, 0).is_err());
    }

    #[test]
    fn single_chunk_when_smaller_than_chunk_size() {
        let original = vec![7u8; 10];
        let chunks = collect(temp_file_with(&original), 64);
        assert_eq!(chunks.len(), 1);
        assert_eq!(&chunks[0][..], &original[..]);
    }

    #[test]
    fn resolve_finalize_chunk_size_falls_back_to_default() {
        assert_eq!(resolve_finalize_chunk_size(None), FILE_MODE_CHUNK_SIZE);
        let configured = NonZeroUsize::new(4096).unwrap();
        assert_eq!(resolve_finalize_chunk_size(Some(configured)), 4096);
    }
}
