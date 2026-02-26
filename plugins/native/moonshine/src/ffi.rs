// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! FFI bindings to the Moonshine C API (moonshine-c-api.h)
//! Based on https://github.com/moonshine-ai/moonshine/blob/main/core/moonshine-c-api.h

use std::os::raw::c_char;

/// Header version constant (MAJOR * 10000 + MINOR * 100 + PATCH).
/// Version 2.0.0 = 20000.
pub const MOONSHINE_HEADER_VERSION: i32 = 20000;

/// Supported model architectures.
pub const MOONSHINE_MODEL_ARCH_TINY: u32 = 0;
pub const MOONSHINE_MODEL_ARCH_BASE: u32 = 1;
pub const MOONSHINE_MODEL_ARCH_TINY_STREAMING: u32 = 2;
pub const MOONSHINE_MODEL_ARCH_BASE_STREAMING: u32 = 3;
pub const MOONSHINE_MODEL_ARCH_SMALL_STREAMING: u32 = 4;
pub const MOONSHINE_MODEL_ARCH_MEDIUM_STREAMING: u32 = 5;

/// Error codes.
pub const MOONSHINE_ERROR_NONE: i32 = 0;

/// Flags.
pub const MOONSHINE_FLAG_FORCE_UPDATE: u32 = 1 << 0;

/// Option passed to `moonshine_load_transcriber_from_files` at creation time.
#[repr(C)]
pub struct TranscriberOption {
    pub name: *const c_char,
    pub value: *const c_char,
}

/// A single line of a transcript.
///
/// All memory referenced by the line objects is owned by the transcriber and is
/// valid until the next call to that transcriber, or until the transcriber is freed.
#[repr(C)]
pub struct TranscriptLine {
    /// UTF-8-encoded transcription text.
    pub text: *const c_char,
    /// The audio data for the current phrase.
    pub audio_data: *const f32,
    /// Number of elements in the audio data array.
    pub audio_data_count: usize,
    /// Time offset from the start of the stream in seconds.
    pub start_time: f32,
    /// Duration of this segment in seconds.
    pub duration: f32,
    /// Stable identifier for the line.
    pub id: u64,
    /// Streaming-only: 0 = speaker still talking, non-zero = complete.
    pub is_complete: i8,
    /// Streaming-only: Whether updated since previous call.
    pub is_updated: i8,
    /// Streaming-only: Whether newly added since previous call.
    pub is_new: i8,
    /// Streaming-only: Whether the text changed since previous call.
    pub has_text_changed: i8,
    /// Whether a speaker ID has been calculated.
    pub has_speaker_id: i8,
    /// The speaker ID for the line.
    pub speaker_id: u64,
    /// What order the speaker appeared in the current transcript.
    pub speaker_index: u32,
    /// Streaming-only: Latency of the last transcription in milliseconds.
    pub last_transcription_latency_ms: u32,
}

/// An entire transcription of an audio stream.
#[repr(C)]
pub struct Transcript {
    /// All lines of the transcript.
    pub lines: *mut TranscriptLine,
    /// Number of lines in the transcript.
    pub line_count: u64,
}

extern "C" {
    /// Returns the loaded moonshine library version.
    #[allow(dead_code)]
    pub fn moonshine_get_version() -> i32;

    /// Converts an error code into a human-readable string.
    pub fn moonshine_error_to_string(error: i32) -> *const c_char;

    /// Loads models from the file system.
    ///
    /// `path` is the root directory containing:
    ///   - encoder_model.ort
    ///   - decoder_model_merged.ort
    ///   - tokenizer.bin
    ///
    /// Returns a non-negative transcriber handle on success, or a negative error code.
    pub fn moonshine_load_transcriber_from_files(
        path: *const c_char,
        model_arch: u32,
        options: *const TranscriberOption,
        options_count: u64,
        moonshine_version: i32,
    ) -> i32;

    /// Transcribes audio without streaming (one-shot).
    ///
    /// Audio data is 16kHz float PCM, between -1.0 and 1.0.
    ///
    /// Returns `MOONSHINE_ERROR_NONE` on success.
    #[allow(dead_code)]
    pub fn moonshine_transcribe_without_streaming(
        transcriber_handle: i32,
        audio_data: *const f32,
        audio_length: u64,
        sample_rate: i32,
        flags: u32,
        out_transcript: *mut *mut Transcript,
    ) -> i32;

    /// Creates a stream associated with a transcriber.
    ///
    /// Returns a non-negative stream handle on success, or a negative error code.
    pub fn moonshine_create_stream(transcriber_handle: i32, flags: u32) -> i32;

    /// Releases the resources used by a stream.
    pub fn moonshine_free_stream(transcriber_handle: i32, stream_handle: i32) -> i32;

    /// Starts a stream. Call before adding audio data.
    ///
    /// Start/stop are supported because there may be discontinuities in the
    /// audio input (e.g. when the user mutes), so we need a way to start fresh.
    ///
    /// Returns zero on success, or a non-zero error code.
    pub fn moonshine_start_stream(transcriber_handle: i32, stream_handle: i32) -> i32;

    /// Stops a stream.
    ///
    /// Returns zero on success, or a non-zero error code.
    pub fn moonshine_stop_stream(transcriber_handle: i32, stream_handle: i32) -> i32;

    /// Adds new audio data to a stream's buffer.
    ///
    /// This function only adds data to the buffer and does no processing,
    /// so it is safe to call frequently even from time-critical threads.
    /// Call `moonshine_transcribe_stream` when you want an updated transcript.
    ///
    /// Audio data is 16kHz float PCM, between -1.0 and 1.0.
    ///
    /// Returns zero on success, or a non-zero error code.
    pub fn moonshine_transcribe_add_audio_to_stream(
        transcriber_handle: i32,
        stream_handle: i32,
        new_audio_data: *const f32,
        audio_length: u64,
        sample_rate: i32,
        flags: u32,
    ) -> i32;

    /// Analyzes all audio data in the stream and returns an updated transcript.
    ///
    /// By default only performs full analysis if there has been more than 200ms
    /// of new samples since the last analysis. Override with `MOONSHINE_FLAG_FORCE_UPDATE`.
    ///
    /// Returns zero on success, or a non-zero error code.
    pub fn moonshine_transcribe_stream(
        transcriber_handle: i32,
        stream_handle: i32,
        flags: u32,
        out_transcript: *mut *mut Transcript,
    ) -> i32;

    /// Frees a transcriber and all associated resources.
    pub fn moonshine_free_transcriber(transcriber_handle: i32);
}
