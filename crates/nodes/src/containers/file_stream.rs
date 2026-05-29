// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Bounded chunked read-back of finalized File-mode temp files.
//!
//! File-mode container muxers write to an on-disk temp file so they can seek and
//! back-patch without holding the whole output in memory. At finalization the
//! temp file must be streamed downstream in bounded chunks (one `Packet::Binary`
//! per chunk) so peak memory stays bounded regardless of output size.

use bytes::Bytes;
use std::io::{Read as _, Seek, SeekFrom};

/// Chunk size used when streaming a finalized File-mode temp file downstream.
pub const FILE_MODE_CHUNK_SIZE: usize = 256 * 1024;

/// Reads a finalized temp file back in bounded chunks.
///
/// Each [`ChunkedFileReader::next_chunk`] call returns at most `chunk_size`
/// bytes, so the caller can emit one packet per chunk without ever allocating a
/// buffer the size of the whole file.
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
                std::io::ErrorKind::InvalidInput,
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
        let chunk = u64::try_from(self.chunk_size).unwrap_or(u64::MAX).min(self.remaining);
        let want = usize::try_from(chunk).map_err(std::io::Error::other)?;
        let mut buf = vec![0u8; want];
        self.file.read_exact(&mut buf)?;
        self.remaining -= chunk;
        Ok(Some(Bytes::from(buf)))
    }
}

#[cfg(test)]
// Tests use unwrap to fail loudly on setup/read errors.
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::io::Write as _;

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
}
