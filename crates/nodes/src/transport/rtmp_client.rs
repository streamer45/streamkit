// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Minimal sans-I/O RTMP publish client.
//!
//! Implements just enough of the RTMP protocol to connect to an RTMP/RTMPS
//! server and publish H.264 video + AAC audio.  No server-side handling,
//! play/subscribe, or AMF3 support.
//!
//! This module replaces the external `shiguredo_rtmp` crate, fixing two
//! spec-compliance issues:
//!
//! 1. **Chunk stream ID assignment** — protocol control on csid 2, commands
//!    and media on csid 3+ (the old library used csid 2 for everything,
//!    which Twitch rejects).
//! 2. **Server-assigned stream ID** — the `createStream` response's stream
//!    ID is stored and used for publish/media (the old library hardcoded 2,
//!    but Twitch assigns 1).
//!
//! Additionally, the client does **not** enforce ACK windows on the send
//! side (matching OBS/FFmpeg behaviour), eliminating the need for the
//! `override_ack_window` hack.

use std::collections::{HashMap, VecDeque};
use std::fmt;

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

/// Error type for the RTMP client module.
#[derive(Debug)]
pub(super) struct Error {
    message: String,
}

impl Error {
    fn new(msg: impl Into<String>) -> Self {
        Self { message: msg.into() }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

// ---------------------------------------------------------------------------
// RtmpUrl
// ---------------------------------------------------------------------------

/// Parsed RTMP URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RtmpUrl {
    pub host: String,
    pub port: u16,
    pub app: String,
    pub stream_name: String,
    pub tls: bool,
}

impl RtmpUrl {
    /// Parse `rtmp[s]://host[:port]/app[/extra_segments]/stream_name`.
    ///
    /// The path is split on the **last** `/` into `app` and `stream_name`.
    /// Default ports: 1935 (rtmp), 443 (rtmps).
    pub fn parse(s: &str) -> Result<Self, Error> {
        let (tls, rest) = if let Some(r) = s.strip_prefix("rtmps://") {
            (true, r)
        } else if let Some(r) = s.strip_prefix("rtmp://") {
            (false, r)
        } else {
            return Err(Error::new("URL must start with rtmp:// or rtmps://"));
        };

        let default_port: u16 = if tls { 443 } else { 1935 };

        // Split host[:port] from /path.
        let (authority, path) = rest.find('/').map_or((rest, ""), |i| (&rest[..i], &rest[i + 1..]));

        let (host, port) = if let Some(colon) = authority.rfind(':') {
            let port_str = &authority[colon + 1..];
            let port = port_str
                .parse::<u16>()
                .map_err(|_| Error::new(format!("Invalid port: {port_str}")))?;
            (authority[..colon].to_string(), port)
        } else {
            (authority.to_string(), default_port)
        };

        if host.is_empty() {
            return Err(Error::new("Empty host"));
        }

        // Split path on last `/` into app and stream_name.
        let (app, stream_name) =
            path.rfind('/').map_or(("", path), |i| (&path[..i], &path[i + 1..]));

        // The "app" is everything before the last segment; if there's only
        // one segment it becomes the stream_name and app is the whole path
        // portion before the stream_name (which would be empty).  But RTMP
        // requires both, so we handle the single-segment case: the single
        // segment is the app with an empty stream_name.
        if app.is_empty() && !stream_name.is_empty() {
            // Single path segment: treat it as app, stream_name empty.
            // The caller (rtmp.rs) appends the stream key separately.
            return Ok(Self {
                host,
                port,
                app: stream_name.to_string(),
                stream_name: String::new(),
                tls,
            });
        }

        if app.is_empty() {
            return Err(Error::new("Empty app name in RTMP URL"));
        }

        Ok(Self { host, port, app: app.to_string(), stream_name: stream_name.to_string(), tls })
    }

    /// Build the `tcUrl` for the RTMP connect command.
    ///
    /// Format: `rtmp[s]://host/app` — deliberately omits the default port
    /// because Twitch returns a degraded response when the port is included.
    fn tc_url(&self) -> String {
        let scheme = if self.tls { "rtmps" } else { "rtmp" };
        let default_port = if self.tls { 443 } else { 1935 };
        if self.port == default_port {
            format!("{scheme}://{}/{}", self.host, self.app)
        } else {
            format!("{scheme}://{}:{}/{}", self.host, self.port, self.app)
        }
    }
}

impl std::str::FromStr for RtmpUrl {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl fmt::Display for RtmpUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let scheme = if self.tls { "rtmps" } else { "rtmp" };
        write!(f, "{scheme}://{}:{}/{}/{}", self.host, self.port, self.app, self.stream_name)
    }
}

// ---------------------------------------------------------------------------
// RtmpTimestamp / RtmpTimestampDelta
// ---------------------------------------------------------------------------

/// RTMP timestamp (milliseconds, u32).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RtmpTimestamp(u32);

impl RtmpTimestamp {
    pub const fn from_millis(ms: u32) -> Self {
        Self(ms)
    }
    pub const fn millis(self) -> u32 {
        self.0
    }
}

/// RTMP timestamp delta (milliseconds, i32).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RtmpTimestampDelta(i32);

impl RtmpTimestampDelta {
    pub const ZERO: Self = Self(0);
    pub const fn millis(self) -> i32 {
        self.0
    }
}

// ---------------------------------------------------------------------------
// Media types (public API for the rtmp.rs node)
// ---------------------------------------------------------------------------

/// Video frame type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum VideoFrameType {
    KeyFrame,
    InterFrame,
}

/// Video codec identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum VideoCodec {
    Avc,
}

/// AVC packet type (H.264).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AvcPacketType {
    SequenceHeader,
    NalUnit,
}

/// Encoded video frame for RTMP publishing.
pub(super) struct VideoFrame {
    pub timestamp: RtmpTimestamp,
    pub composition_timestamp_offset: RtmpTimestampDelta,
    pub frame_type: VideoFrameType,
    pub codec: VideoCodec,
    pub avc_packet_type: Option<AvcPacketType>,
    pub data: Vec<u8>,
}

/// Audio format identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AudioFormat {
    Aac,
}

/// Audio sample rate (FLV header field).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AudioSampleRate {
    Khz44,
}

/// Encoded audio frame for RTMP publishing.
pub(super) struct AudioFrame {
    pub timestamp: RtmpTimestamp,
    pub format: AudioFormat,
    pub sample_rate: AudioSampleRate,
    pub is_8bit_sample: bool,
    pub is_stereo: bool,
    pub is_aac_sequence_header: bool,
    pub data: Vec<u8>,
}

impl AudioFrame {
    /// FLV-spec fixed sample rate for AAC (value ignored by decoder).
    pub const AAC_SAMPLE_RATE: AudioSampleRate = AudioSampleRate::Khz44;
    /// FLV-spec fixed stereo flag for AAC (value ignored by decoder).
    pub const AAC_STEREO: bool = true;
}

/// AVC Sequence Header (`AVCDecoderConfigurationRecord`).
pub(super) struct AvcSequenceHeader {
    pub avc_profile_indication: u8,
    pub profile_compatibility: u8,
    pub avc_level_indication: u8,
    pub length_size_minus_one: u8,
    pub sps_list: Vec<Vec<u8>>,
    pub pps_list: Vec<Vec<u8>>,
}

impl AvcSequenceHeader {
    /// Serialize to `AVCDecoderConfigurationRecord` bytes.
    #[allow(clippy::cast_possible_truncation)]
    pub fn to_bytes(&self) -> Result<Vec<u8>, Error> {
        if self.sps_list.is_empty() {
            return Err(Error::new("AvcSequenceHeader: no SPS"));
        }
        if self.pps_list.is_empty() {
            return Err(Error::new("AvcSequenceHeader: no PPS"));
        }
        if self.sps_list.len() > 31 {
            return Err(Error::new("AvcSequenceHeader: too many SPS (max 31)"));
        }
        if self.pps_list.len() > 255 {
            return Err(Error::new("AvcSequenceHeader: too many PPS (max 255)"));
        }

        let mut buf = Vec::with_capacity(64);
        // configurationVersion = 1
        buf.push(1);
        buf.push(self.avc_profile_indication);
        buf.push(self.profile_compatibility);
        buf.push(self.avc_level_indication);
        // lengthSizeMinusOne (6 bits reserved=0b111111 | 2 bits)
        buf.push(0xFC | (self.length_size_minus_one & 0x03));
        // numOfSequenceParameterSets (3 bits reserved=0b111 | 5 bits count)
        buf.push(0xE0 | (self.sps_list.len() as u8 & 0x1F));
        for sps in &self.sps_list {
            let len =
                u16::try_from(sps.len()).map_err(|_| Error::new("SPS too large for u16 length"))?;
            buf.extend_from_slice(&len.to_be_bytes());
            buf.extend_from_slice(sps);
        }
        // numOfPictureParameterSets
        buf.push(self.pps_list.len() as u8);
        for pps in &self.pps_list {
            let len =
                u16::try_from(pps.len()).map_err(|_| Error::new("PPS too large for u16 length"))?;
            buf.extend_from_slice(&len.to_be_bytes());
            buf.extend_from_slice(pps);
        }
        Ok(buf)
    }
}

// ---------------------------------------------------------------------------
// AMF0 codec (subset)
// ---------------------------------------------------------------------------

/// AMF0 value — only the types needed for RTMP publish commands.
#[derive(Debug, Clone, PartialEq)]
enum Amf0Value {
    Number(f64),
    Boolean(bool),
    String(String),
    Object(Vec<(String, Self)>),
    Null,
}

// AMF0 type markers.
const AMF0_NUMBER: u8 = 0x00;
const AMF0_BOOLEAN: u8 = 0x01;
const AMF0_STRING: u8 = 0x02;
const AMF0_OBJECT: u8 = 0x03;
const AMF0_NULL: u8 = 0x05;
const AMF0_OBJECT_END: [u8; 3] = [0x00, 0x00, 0x09];

/// Encode an AMF0 value, appending bytes to `buf`.
fn amf0_encode(val: &Amf0Value, buf: &mut Vec<u8>) -> Result<(), Error> {
    match val {
        Amf0Value::Number(n) => {
            buf.push(AMF0_NUMBER);
            buf.extend_from_slice(&n.to_be_bytes());
        },
        Amf0Value::Boolean(b) => {
            buf.push(AMF0_BOOLEAN);
            buf.push(u8::from(*b));
        },
        Amf0Value::String(s) => {
            buf.push(AMF0_STRING);
            amf0_encode_string_payload(s, buf)?;
        },
        Amf0Value::Object(props) => {
            buf.push(AMF0_OBJECT);
            for (key, val) in props {
                amf0_encode_string_payload(key, buf)?;
                amf0_encode(val, buf)?;
            }
            buf.extend_from_slice(&AMF0_OBJECT_END);
        },
        Amf0Value::Null => {
            buf.push(AMF0_NULL);
        },
    }
    Ok(())
}

/// Encode an AMF0 string payload (u16 length + UTF-8, no type marker).
fn amf0_encode_string_payload(s: &str, buf: &mut Vec<u8>) -> Result<(), Error> {
    let len = u16::try_from(s.len())
        .map_err(|_| Error::new(format!("AMF0 string too long ({} bytes, max 65535)", s.len())))?;
    buf.extend_from_slice(&len.to_be_bytes());
    buf.extend_from_slice(s.as_bytes());
    Ok(())
}

/// Decode one AMF0 value from a byte slice.
///
/// Returns `(value, bytes_consumed)` or an error.
fn amf0_decode(data: &[u8]) -> Result<(Amf0Value, usize), Error> {
    if data.is_empty() {
        return Err(Error::new("AMF0: unexpected end of data"));
    }

    let marker = data[0];
    let rest = &data[1..];

    match marker {
        AMF0_NUMBER => {
            if rest.len() < 8 {
                return Err(Error::new("AMF0 Number: need 8 bytes"));
            }
            let n = f64::from_be_bytes(
                rest[..8]
                    .try_into()
                    .map_err(|_| Error::new("AMF0 Number: slice conversion failed"))?,
            );
            Ok((Amf0Value::Number(n), 9))
        },
        AMF0_BOOLEAN => {
            if rest.is_empty() {
                return Err(Error::new("AMF0 Boolean: need 1 byte"));
            }
            Ok((Amf0Value::Boolean(rest[0] != 0), 2))
        },
        AMF0_STRING => {
            let (s, consumed) = amf0_decode_string_payload(rest)?;
            Ok((Amf0Value::String(s), 1 + consumed))
        },
        AMF0_OBJECT => {
            let mut props = Vec::new();
            let mut offset = 1; // past the marker
            loop {
                if data.len() < offset + 3 {
                    return Err(Error::new("AMF0 Object: unexpected end"));
                }
                // Check for object-end marker (00 00 09).
                if data[offset] == 0 && data[offset + 1] == 0 && data[offset + 2] == 0x09 {
                    offset += 3;
                    break;
                }
                let (key, key_consumed) = amf0_decode_string_payload(&data[offset..])?;
                offset += key_consumed;
                let (val, val_consumed) = amf0_decode(&data[offset..])?;
                offset += val_consumed;
                props.push((key, val));
            }
            Ok((Amf0Value::Object(props), offset))
        },
        AMF0_NULL => Ok((Amf0Value::Null, 1)),
        _ => Err(Error::new(format!("AMF0: unsupported type marker 0x{marker:02X}"))),
    }
}

/// Decode an AMF0 string payload (u16 length + UTF-8, no type marker).
fn amf0_decode_string_payload(data: &[u8]) -> Result<(String, usize), Error> {
    if data.len() < 2 {
        return Err(Error::new("AMF0 string: need 2 bytes for length"));
    }
    let len = u16::from_be_bytes([data[0], data[1]]) as usize;
    if data.len() < 2 + len {
        return Err(Error::new("AMF0 string: truncated"));
    }
    let s = std::str::from_utf8(&data[2..2 + len])
        .map_err(|e| Error::new(format!("AMF0 string: invalid UTF-8: {e}")))?
        .to_string();
    Ok((s, 2 + len))
}

// ---------------------------------------------------------------------------
// RTMP Messages
// ---------------------------------------------------------------------------

/// A fully decoded inbound RTMP message.
struct InboundMessage {
    #[cfg(test)]
    timestamp: u32,
    msg_type_id: u8,
    #[cfg(test)]
    stream_id: u32,
    payload: Vec<u8>,
}

/// An outbound RTMP message to be chunk-encoded.
struct OutboundMessage {
    csid: u16,
    timestamp: u32,
    msg_type_id: u8,
    stream_id: u32,
    payload: Vec<u8>,
}

// RTMP message type IDs.
const MSG_SET_CHUNK_SIZE: u8 = 1;
const MSG_ABORT: u8 = 2;
const MSG_ACK: u8 = 3;
const MSG_USER_CONTROL: u8 = 4;
const MSG_WIN_ACK_SIZE: u8 = 5;
const MSG_SET_PEER_BANDWIDTH: u8 = 6;
const MSG_AUDIO: u8 = 8;
const MSG_VIDEO: u8 = 9;
const MSG_COMMAND_AMF0: u8 = 20;

// User control event types.
const UC_STREAM_BEGIN: u16 = 0;
const UC_STREAM_EOF: u16 = 1;
const UC_PING_REQUEST: u16 = 6;

// Chunk stream IDs (RTMP spec-compliant assignment).
const CSID_PROTOCOL_CONTROL: u16 = 2;
const CSID_COMMAND: u16 = 3;

/// Chunk stream ID for commands/media on a given message stream.
/// Stream 0 uses csid=3, stream N uses csid=3+N.
#[allow(clippy::cast_possible_truncation)]
fn csid_for_stream(stream_id: u32) -> u16 {
    // Clamp to avoid overflow — in practice stream IDs are small.
    CSID_COMMAND + (stream_id.min(u32::from(u16::MAX) - u32::from(CSID_COMMAND)) as u16)
}

// ---------------------------------------------------------------------------
// Chunk Encoder
// ---------------------------------------------------------------------------

/// Per-csid state for outbound header compression.
#[derive(Default)]
struct ChunkEncoderCsidState {
    prev_timestamp: u32,
    prev_msg_length: u32,
    prev_msg_type_id: u8,
    prev_stream_id: u32,
    prev_timestamp_delta: u32,
    initialized: bool,
}

/// Encodes RTMP messages into chunked wire format.
struct ChunkEncoder {
    chunk_size: u32,
    csid_states: HashMap<u16, ChunkEncoderCsidState>,
}

impl ChunkEncoder {
    fn new() -> Self {
        Self { chunk_size: 128, csid_states: HashMap::new() }
    }

    const fn set_chunk_size(&mut self, size: u32) {
        self.chunk_size = size;
    }

    /// Encode a complete RTMP message into chunks, appending to `out`.
    #[allow(clippy::cast_possible_truncation)]
    fn encode_message(&mut self, msg: &OutboundMessage, out: &mut Vec<u8>) {
        let payload_len = msg.payload.len() as u32;
        let state = self.csid_states.entry(msg.csid).or_default();

        // Determine fmt and compute the timestamp / delta.
        let (fmt, timestamp_field) = if !state.initialized || msg.stream_id != state.prev_stream_id
        {
            // fmt=0: full header.
            (0u8, msg.timestamp)
        } else {
            let delta = msg.timestamp.wrapping_sub(state.prev_timestamp);
            if payload_len == state.prev_msg_length && msg.msg_type_id == state.prev_msg_type_id {
                if delta == state.prev_timestamp_delta {
                    // fmt=3: all fields match including delta.
                    (3u8, delta)
                } else {
                    // fmt=2: only timestamp delta differs.
                    (2u8, delta)
                }
            } else {
                // fmt=1: stream_id matches, but length/type differ.
                (1u8, delta)
            }
        };

        // Update state.
        if fmt == 0 || fmt == 1 {
            state.prev_timestamp_delta = if fmt == 0 { msg.timestamp } else { timestamp_field };
        } else if fmt == 2 {
            state.prev_timestamp_delta = timestamp_field;
        }
        state.prev_timestamp = msg.timestamp;
        state.prev_msg_length = payload_len;
        state.prev_msg_type_id = msg.msg_type_id;
        state.prev_stream_id = msg.stream_id;
        state.initialized = true;

        let extended = timestamp_field >= 0x00FF_FFFF;
        let ts_wire = if extended { 0x00FF_FFFFu32 } else { timestamp_field };

        // Write the first chunk header.
        encode_basic_header(fmt, msg.csid, out);
        encode_message_header(fmt, ts_wire, payload_len, msg.msg_type_id, msg.stream_id, out);
        if extended {
            out.extend_from_slice(&timestamp_field.to_be_bytes());
        }

        // Write payload, splitting at chunk_size boundaries.
        let chunk_size = self.chunk_size as usize;
        let payload = &msg.payload;
        let first_chunk = payload.len().min(chunk_size);
        out.extend_from_slice(&payload[..first_chunk]);

        let mut offset = first_chunk;
        while offset < payload.len() {
            // Continuation chunk: fmt=3 header.
            encode_basic_header(3, msg.csid, out);
            if extended {
                out.extend_from_slice(&timestamp_field.to_be_bytes());
            }
            let end = (offset + chunk_size).min(payload.len());
            out.extend_from_slice(&payload[offset..end]);
            offset = end;
        }
    }
}

/// Encode the basic header (fmt + csid).
fn encode_basic_header(fmt: u8, csid: u16, out: &mut Vec<u8>) {
    let fmt_bits = fmt << 6;
    if csid < 64 {
        #[allow(clippy::cast_possible_truncation)]
        out.push(fmt_bits | (csid as u8));
    } else if csid < 320 {
        out.push(fmt_bits); // csid field = 0 → 2-byte form
        #[allow(clippy::cast_possible_truncation)]
        out.push((csid - 64) as u8);
    } else {
        out.push(fmt_bits | 1); // csid field = 1 → 3-byte form
        let val = csid - 64;
        #[allow(clippy::cast_possible_truncation)]
        {
            out.push(val as u8);
            out.push((val >> 8) as u8);
        }
    }
}

/// Encode the message header portion based on fmt.
fn encode_message_header(
    fmt: u8,
    ts_wire: u32,
    msg_length: u32,
    msg_type_id: u8,
    stream_id: u32,
    out: &mut Vec<u8>,
) {
    match fmt {
        0 => {
            // 11 bytes: timestamp(3) + msg_length(3) + msg_type_id(1) + stream_id(4 LE)
            out.extend_from_slice(&ts_wire.to_be_bytes()[1..4]); // 3 bytes
            out.extend_from_slice(&msg_length.to_be_bytes()[1..4]); // 3 bytes
            out.push(msg_type_id);
            out.extend_from_slice(&stream_id.to_le_bytes()); // 4 bytes LE
        },
        1 => {
            // 7 bytes: timestamp_delta(3) + msg_length(3) + msg_type_id(1)
            out.extend_from_slice(&ts_wire.to_be_bytes()[1..4]);
            out.extend_from_slice(&msg_length.to_be_bytes()[1..4]);
            out.push(msg_type_id);
        },
        2 => {
            // 3 bytes: timestamp_delta(3)
            out.extend_from_slice(&ts_wire.to_be_bytes()[1..4]);
        },
        // fmt=3 and any other value: 0 bytes.
        _ => {},
    }
}

// ---------------------------------------------------------------------------
// Chunk Decoder
// ---------------------------------------------------------------------------

/// Per-csid state for inbound chunk reassembly.
#[derive(Default, Clone)]
struct ChunkDecoderCsidState {
    timestamp: u32,
    msg_length: u32,
    msg_type_id: u8,
    stream_id: u32,
    timestamp_delta: u32,
    payload: Vec<u8>,
    bytes_remaining: u32,
    has_prev: bool,
}

/// Decodes the chunked wire format into complete RTMP messages.
struct ChunkDecoder {
    chunk_size: u32,
    csid_states: HashMap<u16, ChunkDecoderCsidState>,
    buf: Vec<u8>,
}

impl ChunkDecoder {
    fn new() -> Self {
        Self { chunk_size: 128, csid_states: HashMap::new(), buf: Vec::with_capacity(8192) }
    }

    const fn set_chunk_size(&mut self, size: u32) {
        self.chunk_size = size;
    }

    fn push(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    /// Try to decode the next complete message from the buffer.
    ///
    /// Returns `Ok(None)` if there isn't enough data yet.
    #[allow(clippy::cast_possible_truncation)]
    fn decode_message(&mut self) -> Result<Option<InboundMessage>, Error> {
        if self.buf.is_empty() {
            return Ok(None);
        }

        let mut pos = 0;

        // ── Basic header ────────────────────────────────────────────
        if pos >= self.buf.len() {
            return Ok(None);
        }
        let first_byte = self.buf[pos];
        pos += 1;
        let fmt = first_byte >> 6;
        let csid_low = first_byte & 0x3F;

        let csid: u16 = match csid_low {
            0 => {
                // 2-byte form.
                if pos >= self.buf.len() {
                    return Ok(None);
                }
                let c = u16::from(self.buf[pos]) + 64;
                pos += 1;
                c
            },
            1 => {
                // 3-byte form.
                if pos + 1 >= self.buf.len() {
                    return Ok(None);
                }
                let c = u16::from(self.buf[pos]) + u16::from(self.buf[pos + 1]) * 256 + 64;
                pos += 2;
                c
            },
            _ => u16::from(csid_low),
        };

        // ── Message header ──────────────────────────────────────────
        let header_len: usize = match fmt {
            0 => 11,
            1 => 7,
            2 => 3,
            3 => 0,
            _ => return Err(Error::new(format!("Invalid chunk fmt: {fmt}"))),
        };

        if pos + header_len > self.buf.len() {
            return Ok(None); // need more data
        }

        let state = self.csid_states.entry(csid).or_default();

        match fmt {
            0 => {
                let ts = u32::from(self.buf[pos]) << 16
                    | u32::from(self.buf[pos + 1]) << 8
                    | u32::from(self.buf[pos + 2]);
                let ml = u32::from(self.buf[pos + 3]) << 16
                    | u32::from(self.buf[pos + 4]) << 8
                    | u32::from(self.buf[pos + 5]);
                let mt = self.buf[pos + 6];
                let si = u32::from(self.buf[pos + 7])
                    | u32::from(self.buf[pos + 8]) << 8
                    | u32::from(self.buf[pos + 9]) << 16
                    | u32::from(self.buf[pos + 10]) << 24;
                pos += 11;
                state.timestamp = ts;
                state.msg_length = ml;
                state.msg_type_id = mt;
                state.stream_id = si;
                state.timestamp_delta = ts; // for fmt=0, delta equals timestamp
            },
            1 => {
                let td = u32::from(self.buf[pos]) << 16
                    | u32::from(self.buf[pos + 1]) << 8
                    | u32::from(self.buf[pos + 2]);
                let ml = u32::from(self.buf[pos + 3]) << 16
                    | u32::from(self.buf[pos + 4]) << 8
                    | u32::from(self.buf[pos + 5]);
                let mt = self.buf[pos + 6];
                pos += 7;
                state.timestamp_delta = td;
                if state.has_prev {
                    state.timestamp = state.timestamp.wrapping_add(td);
                } else {
                    state.timestamp = td;
                }
                state.msg_length = ml;
                state.msg_type_id = mt;
                // stream_id inherited
            },
            2 => {
                let td = u32::from(self.buf[pos]) << 16
                    | u32::from(self.buf[pos + 1]) << 8
                    | u32::from(self.buf[pos + 2]);
                pos += 3;
                state.timestamp_delta = td;
                if state.has_prev {
                    state.timestamp = state.timestamp.wrapping_add(td);
                } else {
                    state.timestamp = td;
                }
                // msg_length, msg_type_id, stream_id inherited
            },
            3 => {
                // All inherited. Apply delta for continuation of a new message
                // (not a continuation chunk of the same message).
                if state.bytes_remaining == 0 && state.has_prev {
                    state.timestamp = state.timestamp.wrapping_add(state.timestamp_delta);
                }
            },
            _ => unreachable!(),
        }

        // Extended timestamp.
        let is_extended = if fmt == 0 {
            state.timestamp == 0x00FF_FFFF
        } else {
            state.timestamp_delta == 0x00FF_FFFF
        };

        if is_extended {
            if pos + 4 > self.buf.len() {
                return Ok(None);
            }
            let ext = u32::from_be_bytes([
                self.buf[pos],
                self.buf[pos + 1],
                self.buf[pos + 2],
                self.buf[pos + 3],
            ]);
            pos += 4;
            state.timestamp = if fmt == 0 {
                ext
            } else {
                // For fmt 1/2/3 with extended timestamp, the ext field
                // replaces the delta.
                state.timestamp.wrapping_sub(state.timestamp_delta).wrapping_add(ext)
            };
            state.timestamp_delta = ext;
        }

        // ── Payload ─────────────────────────────────────────────────
        // If bytes_remaining == 0, this is the first chunk of a new message.
        if state.bytes_remaining == 0 {
            state.payload.clear();
            state.bytes_remaining = state.msg_length;
        }

        let chunk_data_len = (state.bytes_remaining).min(self.chunk_size) as usize;
        if pos + chunk_data_len > self.buf.len() {
            return Ok(None); // need more data
        }

        state.payload.extend_from_slice(&self.buf[pos..pos + chunk_data_len]);
        state.bytes_remaining -= chunk_data_len as u32;
        pos += chunk_data_len;

        // Consume the bytes we've processed.
        self.buf.drain(..pos);

        // Check if the message is complete.
        if state.bytes_remaining == 0 {
            state.has_prev = true;
            let msg = InboundMessage {
                #[cfg(test)]
                timestamp: state.timestamp,
                msg_type_id: state.msg_type_id,
                #[cfg(test)]
                stream_id: state.stream_id,
                payload: std::mem::take(&mut state.payload),
            };
            Ok(Some(msg))
        } else {
            Ok(None) // message not yet fully assembled
        }
    }
}

// ---------------------------------------------------------------------------
// Handshake
// ---------------------------------------------------------------------------

/// Client-side RTMP handshake state machine.
struct Handshake {
    state: HandshakeState,
    recv_buf: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HandshakeState {
    WaitingForS0S1,
    WaitingForS2,
    Complete,
}

const HANDSHAKE_SIZE: usize = 1536;

impl Handshake {
    /// Create a new handshake and return `(self, c0c1_bytes)`.
    ///
    /// C0 = version byte (0x03).
    /// C1 = 1536 bytes: timestamp(4) + zero(4) + random(1528).
    fn new() -> (Self, Vec<u8>) {
        let mut c1 = vec![0u8; HANDSHAKE_SIZE];
        // Timestamp = 0 (first 4 bytes already zero).
        // Version = 0 (next 4 bytes already zero).
        // Random data for bytes 8..1536.
        fill_random(&mut c1[8..]);

        let mut c0c1 = Vec::with_capacity(1 + HANDSHAKE_SIZE);
        c0c1.push(0x03); // RTMP version
        c0c1.extend_from_slice(&c1);

        (
            Self {
                state: HandshakeState::WaitingForS0S1,
                recv_buf: Vec::with_capacity(1 + HANDSHAKE_SIZE * 2),
            },
            c0c1,
        )
    }

    /// Feed received bytes from the server.
    ///
    /// Returns:
    /// - `None` — need more data.
    /// - `Some((c2, leftover))` — handshake complete, send C2.  `leftover`
    ///   contains any post-S2 bytes that arrived in the same TCP segment
    ///   and must be forwarded to the chunk decoder.
    fn feed(&mut self, data: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
        self.recv_buf.extend_from_slice(data);

        match self.state {
            HandshakeState::WaitingForS0S1 => {
                // Need S0 (1 byte) + S1 (1536 bytes) = 1537 bytes.
                if self.recv_buf.len() < 1 + HANDSHAKE_SIZE {
                    return None;
                }

                let s0 = self.recv_buf[0];
                if s0 != 0x03 {
                    tracing::warn!(s0, "RTMP server version is not 3, continuing anyway");
                }

                // S1 is bytes 1..1537.  We'll need it for C2.
                // Move to waiting for S2.
                self.state = HandshakeState::WaitingForS2;

                // Check if S2 is also already here.
                if self.recv_buf.len() > HANDSHAKE_SIZE * 2 {
                    return Some(self.complete_handshake());
                }
                None
            },
            HandshakeState::WaitingForS2 => {
                if self.recv_buf.len() <= HANDSHAKE_SIZE * 2 {
                    return None;
                }
                Some(self.complete_handshake())
            },
            HandshakeState::Complete => None,
        }
    }

    /// Validate S2 and produce C2, returning any leftover bytes.
    fn complete_handshake(&mut self) -> (Vec<u8>, Vec<u8>) {
        // C2 = echo of S1 (bytes 1..=HANDSHAKE_SIZE of recv_buf).
        let s1 = &self.recv_buf[1..=HANDSHAKE_SIZE];
        let c2 = s1.to_vec();

        // Bytes beyond S0(1) + S1(1536) + S2(1536) = 3073 are
        // post-handshake protocol messages (e.g. WinAckSize,
        // SetPeerBandwidth) that the server pipelined in the same
        // TCP segment.  Return them so the caller can forward them
        // to the chunk decoder.
        let handshake_total = 1 + HANDSHAKE_SIZE * 2;
        let leftover = if self.recv_buf.len() > handshake_total {
            self.recv_buf[handshake_total..].to_vec()
        } else {
            Vec::new()
        };

        self.state = HandshakeState::Complete;
        // Free the receive buffer — no longer needed.
        self.recv_buf = Vec::new();

        (c2, leftover)
    }
}

/// Fill a buffer with pseudo-random bytes.
///
/// Uses a simple xorshift64 PRNG seeded from the current timestamp to avoid
/// all-zero handshakes (which some servers may fingerprint).  Cryptographic
/// strength is not required here.
fn fill_random(buf: &mut [u8]) {
    // Seed from the current time.  We mix in a fixed constant to avoid
    // degenerate seeds (e.g. zero).
    let mut state: u64 = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(
        0x517E_A45D_1234_5678,
        |d| {
            #[allow(clippy::cast_possible_truncation)]
            // Truncation is intentional: we only need 64 bits of entropy for a PRNG seed.
            let nanos = d.as_nanos() as u64;
            nanos
        },
    );
    if state == 0 {
        state = 0x517E_A45D_1234_5678;
    }
    for byte in buf.iter_mut() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        #[allow(clippy::cast_possible_truncation)]
        {
            *byte = state as u8;
        }
    }
}

// ---------------------------------------------------------------------------
// Connection State
// ---------------------------------------------------------------------------

/// RTMP connection states (publish-client subset).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RtmpConnectionState {
    Handshaking,
    Connecting,
    Connected,
    MediaStreamCreated,
    PublishPending,
    Publishing,
    Disconnecting,
}

impl fmt::Display for RtmpConnectionState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Handshaking => f.write_str("Handshaking"),
            Self::Connecting => f.write_str("Connecting"),
            Self::Connected => f.write_str("Connected"),
            Self::MediaStreamCreated => f.write_str("MediaStreamCreated"),
            Self::PublishPending => f.write_str("PublishPending"),
            Self::Publishing => f.write_str("Publishing"),
            Self::Disconnecting => f.write_str("Disconnecting"),
        }
    }
}

/// Events emitted by the connection state machine.
#[derive(Debug)]
pub(super) enum RtmpConnectionEvent {
    StateChanged(RtmpConnectionState),
    DisconnectedByPeer { reason: String },
}

// ---------------------------------------------------------------------------
// RtmpPublishClientConnection
// ---------------------------------------------------------------------------

/// Sans-I/O RTMP publish client connection.
///
/// Manages the full lifecycle from handshake through publish, providing
/// the same API surface as the previous `shiguredo_rtmp` library.
pub(super) struct RtmpPublishClientConnection {
    url: RtmpUrl,
    state: RtmpConnectionState,
    handshake: Option<Handshake>,
    encoder: ChunkEncoder,
    decoder: ChunkDecoder,
    send_buf: Vec<u8>,
    events: VecDeque<RtmpConnectionEvent>,
    /// Server-assigned stream ID from createStream `_result`.
    media_stream_id: u32,
    /// Transaction ID counter for AMF0 commands.
    next_transaction_id: f64,
    /// Total bytes received (for ACK tracking).
    total_bytes_received: u64,
    /// Peer's requested ACK window size.
    peer_ack_window_size: u32,
    /// Byte count at which we last sent an ACK.
    last_ack_sent_at: u64,
    /// Chunk size we announce to the server.
    local_chunk_size: u32,
}

impl fmt::Debug for RtmpPublishClientConnection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RtmpPublishClientConnection")
            .field("state", &self.state)
            .field("url", &self.url.to_string())
            .finish_non_exhaustive()
    }
}

impl RtmpPublishClientConnection {
    /// The chunk size we announce to the server (4096 bytes — matches
    /// OBS/FFmpeg; the RTMP default of 128 is too small for video).
    const LOCAL_CHUNK_SIZE: u32 = 4096;

    /// Maximum send buffer size (8 MB).  If the TCP socket stalls and
    /// the buffer exceeds this, we refuse to enqueue more media so the
    /// caller can detect backpressure and disconnect gracefully.
    const MAX_SEND_BUF: usize = 8 * 1024 * 1024;

    /// Create a new RTMP publish client.  C0+C1 are queued in the send
    /// buffer immediately.
    pub fn new(url: RtmpUrl) -> Self {
        let (handshake, c0c1) = Handshake::new();

        Self {
            url,
            state: RtmpConnectionState::Handshaking,
            handshake: Some(handshake),
            encoder: ChunkEncoder::new(),
            decoder: ChunkDecoder::new(),
            send_buf: c0c1,
            events: VecDeque::new(),
            media_stream_id: 0,
            next_transaction_id: 1.0,
            total_bytes_received: 0,
            peer_ack_window_size: 0,
            last_ack_sent_at: 0,
            local_chunk_size: Self::LOCAL_CHUNK_SIZE,
        }
    }

    /// Feed received bytes from the server.  Drives the state machine
    /// (handshake → connect → createStream → publish).
    pub fn feed_recv_buf(&mut self, buf: &[u8]) -> Result<(), Error> {
        // ── Handshake phase ─────────────────────────────────────────
        // ACK sequence numbers are based on post-handshake bytes only
        // (RTMP spec §5.4), so we defer the counter increment.
        if let Some(ref mut hs) = self.handshake {
            if let Some((c2, leftover)) = hs.feed(buf) {
                self.send_buf.extend_from_slice(&c2);

                // Handshake complete — send the RTMP connect sequence.
                self.handshake = None;
                self.send_connect_sequence()?;

                // Forward any post-S2 bytes (e.g. WinAckSize,
                // SetPeerBandwidth pipelined in the same TCP segment)
                // to the chunk decoder so they aren't silently lost.
                if !leftover.is_empty() {
                    self.total_bytes_received += leftover.len() as u64;
                    self.decoder.push(&leftover);
                    while let Some(msg) = self.decoder.decode_message()? {
                        self.handle_message(&msg)?;
                    }
                    self.maybe_send_ack();
                }
                return Ok(());
            }
            // Still handshaking, need more data.
            return Ok(());
        }

        // ── Post-handshake: decode chunks ───────────────────────────
        self.total_bytes_received += buf.len() as u64;
        self.decoder.push(buf);
        while let Some(msg) = self.decoder.decode_message()? {
            self.handle_message(&msg)?;
        }

        // ── ACK tracking ────────────────────────────────────────────
        self.maybe_send_ack();

        Ok(())
    }

    /// Bytes waiting to be sent to the server.
    pub fn send_buf(&self) -> &[u8] {
        &self.send_buf
    }

    /// Mark `n` bytes as sent.
    pub fn advance_send_buf(&mut self, n: usize) {
        self.send_buf.drain(..n);
    }

    /// Current connection state.
    pub const fn state(&self) -> RtmpConnectionState {
        self.state
    }

    /// Send a video frame (only valid in `Publishing` state).
    #[allow(clippy::cast_possible_truncation)]
    pub fn send_video(&mut self, frame: &VideoFrame) -> Result<(), Error> {
        if self.state != RtmpConnectionState::Publishing {
            return Err(Error::new(format!("Cannot send video in state {}", self.state)));
        }
        if self.send_buf.len() > Self::MAX_SEND_BUF {
            return Err(Error::new(format!(
                "Send buffer exceeded {} bytes — backpressure (TCP stall?)",
                Self::MAX_SEND_BUF
            )));
        }

        // Build the FLV video tag payload.
        let mut payload = Vec::with_capacity(5 + frame.data.len());

        // FLV video header byte: frame_type(4 bits) | codec_id(4 bits)
        let frame_type_nibble: u8 = match frame.frame_type {
            VideoFrameType::KeyFrame => 1,
            VideoFrameType::InterFrame => 2,
        };
        let codec_nibble: u8 = match frame.codec {
            VideoCodec::Avc => 7,
        };
        payload.push((frame_type_nibble << 4) | codec_nibble);

        // AVC packet type + composition time offset (3 bytes, signed 24-bit)
        if let Some(ref pkt_type) = frame.avc_packet_type {
            payload.push(match pkt_type {
                AvcPacketType::SequenceHeader => 0,
                AvcPacketType::NalUnit => 1,
            });
            let cto = frame.composition_timestamp_offset.millis();
            let cto_bytes = cto.to_be_bytes();
            // 24-bit signed: take lower 3 bytes of i32
            payload.extend_from_slice(&cto_bytes[1..4]);
        }

        payload.extend_from_slice(&frame.data);

        let msg = OutboundMessage {
            csid: csid_for_stream(self.media_stream_id),
            timestamp: frame.timestamp.millis(),
            msg_type_id: MSG_VIDEO,
            stream_id: self.media_stream_id,
            payload,
        };
        self.encoder.encode_message(&msg, &mut self.send_buf);
        Ok(())
    }

    /// Send an audio frame (only valid in `Publishing` state).
    pub fn send_audio(&mut self, frame: &AudioFrame) -> Result<(), Error> {
        if self.state != RtmpConnectionState::Publishing {
            return Err(Error::new(format!("Cannot send audio in state {}", self.state)));
        }
        if self.send_buf.len() > Self::MAX_SEND_BUF {
            return Err(Error::new(format!(
                "Send buffer exceeded {} bytes — backpressure (TCP stall?)",
                Self::MAX_SEND_BUF
            )));
        }

        // Build the FLV audio tag payload.
        let mut payload = Vec::with_capacity(2 + frame.data.len());

        // FLV audio header byte:
        // soundFormat(4) | soundRate(2) | soundSize(1) | soundType(1)
        let format_nibble: u8 = match frame.format {
            AudioFormat::Aac => 10,
        };
        let rate_bits: u8 = match frame.sample_rate {
            AudioSampleRate::Khz44 => 3, // 44 kHz
        };
        let size_bit: u8 = u8::from(!frame.is_8bit_sample); // 0=8bit, 1=16bit
        let type_bit: u8 = u8::from(frame.is_stereo);
        payload.push((format_nibble << 4) | (rate_bits << 2) | (size_bit << 1) | type_bit);

        // AAC packet type: 0 = sequence header, 1 = raw
        if matches!(frame.format, AudioFormat::Aac) {
            payload.push(u8::from(!frame.is_aac_sequence_header));
        }

        payload.extend_from_slice(&frame.data);

        let msg = OutboundMessage {
            csid: csid_for_stream(self.media_stream_id),
            timestamp: frame.timestamp.millis(),
            msg_type_id: MSG_AUDIO,
            stream_id: self.media_stream_id,
            payload,
        };
        self.encoder.encode_message(&msg, &mut self.send_buf);
        Ok(())
    }

    /// Retrieve the next event, if any.
    pub fn next_event(&mut self) -> Option<RtmpConnectionEvent> {
        self.events.pop_front()
    }

    // -------------------------------------------------------------------
    // Internal: connect sequence
    // -------------------------------------------------------------------

    /// Send the initial RTMP connect command sequence after handshake.
    fn send_connect_sequence(&mut self) -> Result<(), Error> {
        // 1. WinAckSize (server should ACK every 2.5 MB).
        self.send_protocol_message(MSG_WIN_ACK_SIZE, &2_500_000u32.to_be_bytes());

        // 2. SetChunkSize.
        self.send_protocol_message(MSG_SET_CHUNK_SIZE, &self.local_chunk_size.to_be_bytes());
        self.encoder.set_chunk_size(self.local_chunk_size);

        // 3. connect command.
        let tid = self.next_tid();
        let tc_url = self.url.tc_url();
        let app = self.url.app.clone();

        let mut payload = Vec::with_capacity(256);
        amf0_encode(&Amf0Value::String("connect".to_string()), &mut payload)?;
        amf0_encode(&Amf0Value::Number(tid), &mut payload)?;
        amf0_encode(
            &Amf0Value::Object(vec![
                ("app".to_string(), Amf0Value::String(app)),
                ("type".to_string(), Amf0Value::String("nonprivate".to_string())),
                ("flashVer".to_string(), Amf0Value::String("FMLE/3.0".to_string())),
                ("tcUrl".to_string(), Amf0Value::String(tc_url)),
            ]),
            &mut payload,
        )?;

        let msg = OutboundMessage {
            csid: CSID_COMMAND,
            timestamp: 0,
            msg_type_id: MSG_COMMAND_AMF0,
            stream_id: 0,
            payload,
        };
        self.encoder.encode_message(&msg, &mut self.send_buf);

        self.set_state(RtmpConnectionState::Connecting);
        Ok(())
    }

    // -------------------------------------------------------------------
    // Internal: message handling
    // -------------------------------------------------------------------

    /// Handle a fully assembled inbound RTMP message.
    fn handle_message(&mut self, msg: &InboundMessage) -> Result<(), Error> {
        match msg.msg_type_id {
            MSG_SET_CHUNK_SIZE => {
                if msg.payload.len() >= 4 {
                    let size = u32::from_be_bytes([
                        msg.payload[0],
                        msg.payload[1],
                        msg.payload[2],
                        msg.payload[3],
                    ]) & 0x7FFF_FFFF; // high bit must be 0
                    tracing::debug!(chunk_size = size, "Server SetChunkSize");
                    self.decoder.set_chunk_size(size);
                }
            },
            MSG_ABORT => {
                // Abort message for a chunk stream — clear partial state.
                if msg.payload.len() >= 4 {
                    let abort_csid = u32::from_be_bytes([
                        msg.payload[0],
                        msg.payload[1],
                        msg.payload[2],
                        msg.payload[3],
                    ]);
                    #[allow(clippy::cast_possible_truncation)]
                    let csid = abort_csid as u16;
                    if let Some(state) = self.decoder.csid_states.get_mut(&csid) {
                        state.payload.clear();
                        state.bytes_remaining = 0;
                    }
                }
            },
            MSG_ACK => {
                // Server acknowledgement — we don't enforce ACK windows
                // on the send side, so just log it.
                tracing::debug!("Server ACK received");
            },
            MSG_USER_CONTROL => self.handle_user_control(&msg.payload),
            MSG_WIN_ACK_SIZE => {
                if msg.payload.len() >= 4 {
                    let size = u32::from_be_bytes([
                        msg.payload[0],
                        msg.payload[1],
                        msg.payload[2],
                        msg.payload[3],
                    ]);
                    tracing::debug!(window_size = size, "Server WinAckSize");
                    self.peer_ack_window_size = size;
                }
            },
            MSG_SET_PEER_BANDWIDTH => {
                if msg.payload.len() >= 5 {
                    let size = u32::from_be_bytes([
                        msg.payload[0],
                        msg.payload[1],
                        msg.payload[2],
                        msg.payload[3],
                    ]);
                    tracing::debug!(
                        window_size = size,
                        limit_type = msg.payload[4],
                        "Server SetPeerBandwidth"
                    );
                    // Respond with WinAckSize to acknowledge.
                    self.send_protocol_message(MSG_WIN_ACK_SIZE, &size.to_be_bytes());
                    self.peer_ack_window_size = size;
                }
            },
            MSG_COMMAND_AMF0 => self.handle_command(&msg.payload)?,
            MSG_AUDIO | MSG_VIDEO => {
                // We're a publisher, not a subscriber — ignore inbound media.
            },
            _ => {
                tracing::debug!(msg_type = msg.msg_type_id, "Ignoring unknown RTMP message type");
            },
        }
        Ok(())
    }

    /// Handle a User Control event message (type 4).
    fn handle_user_control(&mut self, payload: &[u8]) {
        if payload.len() < 2 {
            return;
        }
        let event_type = u16::from_be_bytes([payload[0], payload[1]]);

        match event_type {
            UC_STREAM_BEGIN => {
                tracing::debug!("User control: StreamBegin");
            },
            UC_STREAM_EOF => {
                tracing::debug!("User control: StreamEof");
            },
            UC_PING_REQUEST => {
                // Respond with PingResponse (event type 7).
                if payload.len() >= 6 {
                    let mut response = Vec::with_capacity(6);
                    response.extend_from_slice(&7u16.to_be_bytes()); // PingResponse
                    response.extend_from_slice(&payload[2..6]); // echo timestamp
                    self.send_protocol_message(MSG_USER_CONTROL, &response);
                    tracing::debug!("Responded to PingRequest");
                }
            },
            _ => {
                tracing::debug!(event_type, "User control event ignored");
            },
        }
    }

    /// Handle an AMF0 command message.
    fn handle_command(&mut self, payload: &[u8]) -> Result<(), Error> {
        // Decode command name.
        let (name_val, mut offset) = amf0_decode(payload)?;
        let name = match &name_val {
            Amf0Value::String(s) => s.as_str(),
            _ => return Ok(()), // not a command
        };

        // Decode transaction ID.
        let (tid_val, consumed) = amf0_decode(&payload[offset..])?;
        offset += consumed;
        let _tid = match &tid_val {
            Amf0Value::Number(n) => *n,
            _ => 0.0,
        };

        match name {
            "_result" => self.handle_result(&payload[offset..])?,
            "_error" => self.handle_error(&payload[offset..]),
            "onStatus" => self.handle_on_status(&payload[offset..]),
            _ => {
                tracing::debug!(command = name, "Ignoring unknown RTMP command");
            },
        }

        Ok(())
    }

    /// Handle a `_result` response.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn handle_result(&mut self, payload: &[u8]) -> Result<(), Error> {
        match self.state {
            RtmpConnectionState::Connecting => {
                // connect _result — success.
                tracing::info!("RTMP connect succeeded");
                self.set_state(RtmpConnectionState::Connected);

                // Send createStream.
                let tid = self.next_tid();
                let mut cmd_payload = Vec::with_capacity(32);
                amf0_encode(&Amf0Value::String("createStream".to_string()), &mut cmd_payload)?;
                amf0_encode(&Amf0Value::Number(tid), &mut cmd_payload)?;
                amf0_encode(&Amf0Value::Null, &mut cmd_payload)?;

                let msg = OutboundMessage {
                    csid: CSID_COMMAND,
                    timestamp: 0,
                    msg_type_id: MSG_COMMAND_AMF0,
                    stream_id: 0,
                    payload: cmd_payload,
                };
                self.encoder.encode_message(&msg, &mut self.send_buf);
            },
            RtmpConnectionState::Connected => {
                // createStream _result — extract stream ID.
                // The result payload is: Null (properties) + Number (stream_id).
                let mut off = 0;
                // Skip the Null/Object properties field.
                if !payload.is_empty() {
                    let (_, consumed) = amf0_decode(payload)?;
                    off += consumed;
                }
                // Read the stream ID.
                if off < payload.len() {
                    let (val, _) = amf0_decode(&payload[off..])?;
                    if let Amf0Value::Number(n) = val {
                        self.media_stream_id = n as u32;
                        tracing::info!(stream_id = self.media_stream_id, "createStream succeeded");
                    }
                }

                self.set_state(RtmpConnectionState::MediaStreamCreated);

                // Send publish command on the media stream's csid.
                let tid = self.next_tid();
                let stream_name = self.url.stream_name.clone();
                let mut cmd_payload = Vec::with_capacity(64);
                amf0_encode(&Amf0Value::String("publish".to_string()), &mut cmd_payload)?;
                amf0_encode(&Amf0Value::Number(tid), &mut cmd_payload)?;
                amf0_encode(&Amf0Value::Null, &mut cmd_payload)?;
                amf0_encode(&Amf0Value::String(stream_name), &mut cmd_payload)?;
                amf0_encode(&Amf0Value::String("live".to_string()), &mut cmd_payload)?;

                let msg = OutboundMessage {
                    csid: csid_for_stream(self.media_stream_id),
                    timestamp: 0,
                    msg_type_id: MSG_COMMAND_AMF0,
                    stream_id: self.media_stream_id,
                    payload: cmd_payload,
                };
                self.encoder.encode_message(&msg, &mut self.send_buf);

                self.set_state(RtmpConnectionState::PublishPending);
            },
            _ => {
                tracing::debug!(state = %self.state, "Unexpected _result");
            },
        }
        Ok(())
    }

    /// Handle a `_error` response.
    fn handle_error(&mut self, payload: &[u8]) {
        // Try to extract a description.
        let desc = extract_info_description(payload).unwrap_or_else(|| "unknown error".to_string());
        tracing::warn!(description = %desc, state = %self.state, "RTMP _error");
        self.events.push_back(RtmpConnectionEvent::DisconnectedByPeer { reason: desc });
        self.set_state(RtmpConnectionState::Disconnecting);
    }

    /// Handle an `onStatus` notification.
    fn handle_on_status(&mut self, payload: &[u8]) {
        // Skip Null (command object), then decode the info object.
        let mut off = 0;
        if !payload.is_empty() {
            if let Ok((_, consumed)) = amf0_decode(payload) {
                off += consumed;
            }
        }

        let code = if off < payload.len() {
            if let Ok((val, _)) = amf0_decode(&payload[off..]) {
                extract_object_field(&val, "code")
            } else {
                None
            }
        } else {
            None
        };

        let code_str = code.as_deref().unwrap_or("");
        tracing::info!(code = code_str, state = %self.state, "onStatus");

        match code_str {
            "NetStream.Publish.Start" => {
                self.set_state(RtmpConnectionState::Publishing);
            },
            s if s.contains("Error") || s.contains("Failed") || s.contains("Rejected") => {
                let desc =
                    extract_info_description(payload).unwrap_or_else(|| code_str.to_string());
                self.events.push_back(RtmpConnectionEvent::DisconnectedByPeer { reason: desc });
                self.set_state(RtmpConnectionState::Disconnecting);
            },
            _ => {
                // Other status codes (e.g. NetStream.Play.Start) — ignore.
            },
        }
    }

    // -------------------------------------------------------------------
    // Internal: helpers
    // -------------------------------------------------------------------

    /// Set state and emit a `StateChanged` event.
    fn set_state(&mut self, new_state: RtmpConnectionState) {
        if self.state != new_state {
            tracing::debug!(from = %self.state, to = %new_state, "RTMP state transition");
            self.state = new_state;
            self.events.push_back(RtmpConnectionEvent::StateChanged(new_state));
        }
    }

    /// Allocate the next transaction ID.
    fn next_tid(&mut self) -> f64 {
        let tid = self.next_transaction_id;
        self.next_transaction_id += 1.0;
        tid
    }

    /// Send a protocol control message on csid=2, stream_id=0.
    fn send_protocol_message(&mut self, msg_type_id: u8, payload: &[u8]) {
        let msg = OutboundMessage {
            csid: CSID_PROTOCOL_CONTROL,
            timestamp: 0,
            msg_type_id,
            stream_id: 0,
            payload: payload.to_vec(),
        };
        self.encoder.encode_message(&msg, &mut self.send_buf);
    }

    /// Send an ACK if we've received enough bytes since the last one.
    fn maybe_send_ack(&mut self) {
        if self.peer_ack_window_size == 0 {
            return;
        }
        let since_last = self.total_bytes_received - self.last_ack_sent_at;
        if since_last >= u64::from(self.peer_ack_window_size) {
            #[allow(clippy::cast_possible_truncation)]
            let seq = self.total_bytes_received as u32;
            self.send_protocol_message(MSG_ACK, &seq.to_be_bytes());
            self.last_ack_sent_at = self.total_bytes_received;
        }
    }
}

/// Extract the "description" field from an AMF0 info object payload.
///
/// The payload typically starts with Null (command object) then an Object
/// containing `code`, `level`, `description` fields.
fn extract_info_description(payload: &[u8]) -> Option<String> {
    let mut off = 0;
    // Skip Null/command object.
    if !payload.is_empty() {
        let (_, consumed) = amf0_decode(payload).ok()?;
        off += consumed;
    }
    if off >= payload.len() {
        return None;
    }
    let (val, _) = amf0_decode(&payload[off..]).ok()?;
    extract_object_field(&val, "description")
}

/// Extract a string field from an AMF0 Object value.
fn extract_object_field(val: &Amf0Value, field: &str) -> Option<String> {
    if let Amf0Value::Object(props) = val {
        for (key, v) in props {
            if key == field {
                if let Amf0Value::String(s) = v {
                    return Some(s.clone());
                }
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    // ── URL parsing ─────────────────────────────────────────────────

    #[test]
    fn parse_rtmp_url_basic() {
        let url = RtmpUrl::parse("rtmp://live.example.com/app/stream_key").unwrap();
        assert_eq!(url.host, "live.example.com");
        assert_eq!(url.port, 1935);
        assert_eq!(url.app, "app");
        assert_eq!(url.stream_name, "stream_key");
        assert!(!url.tls);
    }

    #[test]
    fn parse_rtmps_url_with_port() {
        let url = RtmpUrl::parse("rtmps://live.twitch.tv:8443/app/key").unwrap();
        assert_eq!(url.host, "live.twitch.tv");
        assert_eq!(url.port, 8443);
        assert_eq!(url.app, "app");
        assert_eq!(url.stream_name, "key");
        assert!(url.tls);
    }

    #[test]
    fn parse_rtmps_default_port() {
        let url = RtmpUrl::parse("rtmps://live.twitch.tv/app/key").unwrap();
        assert_eq!(url.port, 443);
    }

    #[test]
    fn parse_rtmp_multi_segment_path() {
        let url = RtmpUrl::parse("rtmp://host/live/extra/stream_key").unwrap();
        assert_eq!(url.app, "live/extra");
        assert_eq!(url.stream_name, "stream_key");
    }

    #[test]
    fn parse_rtmp_single_segment_is_app() {
        // When there's only one path segment, it's the app name.
        // Stream name will be appended by the caller (resolve_rtmp_url).
        let url = RtmpUrl::parse("rtmp://host/live2").unwrap();
        assert_eq!(url.app, "live2");
        assert_eq!(url.stream_name, "");
    }

    #[test]
    fn parse_rtmp_invalid_scheme() {
        assert!(RtmpUrl::parse("http://host/app/key").is_err());
    }

    #[test]
    fn parse_rtmp_empty_host() {
        assert!(RtmpUrl::parse("rtmp:///app/key").is_err());
    }

    #[test]
    fn tc_url_omits_default_port() {
        let url = RtmpUrl::parse("rtmp://a.rtmp.youtube.com/live2/key").unwrap();
        assert_eq!(url.tc_url(), "rtmp://a.rtmp.youtube.com/live2");
    }

    #[test]
    fn tc_url_includes_custom_port() {
        let url = RtmpUrl::parse("rtmp://host:9999/app/key").unwrap();
        assert_eq!(url.tc_url(), "rtmp://host:9999/app");
    }

    #[test]
    fn url_from_str_works() {
        let url: RtmpUrl = "rtmp://host/app/key".parse().unwrap();
        assert_eq!(url.host, "host");
    }

    // ── AMF0 ────────────────────────────────────────────────────────

    #[test]
    fn amf0_number_roundtrip() {
        let val = Amf0Value::Number(42.5);
        let mut buf = Vec::new();
        amf0_encode(&val, &mut buf).unwrap();
        let (decoded, consumed) = amf0_decode(&buf).unwrap();
        assert_eq!(decoded, val);
        assert_eq!(consumed, buf.len());
    }

    #[test]
    fn amf0_string_roundtrip() {
        let val = Amf0Value::String("hello RTMP".to_string());
        let mut buf = Vec::new();
        amf0_encode(&val, &mut buf).unwrap();
        let (decoded, consumed) = amf0_decode(&buf).unwrap();
        assert_eq!(decoded, val);
        assert_eq!(consumed, buf.len());
    }

    #[test]
    fn amf0_boolean_roundtrip() {
        for b in [true, false] {
            let val = Amf0Value::Boolean(b);
            let mut buf = Vec::new();
            amf0_encode(&val, &mut buf).unwrap();
            let (decoded, consumed) = amf0_decode(&buf).unwrap();
            assert_eq!(decoded, val);
            assert_eq!(consumed, buf.len());
        }
    }

    #[test]
    fn amf0_null_roundtrip() {
        let val = Amf0Value::Null;
        let mut buf = Vec::new();
        amf0_encode(&val, &mut buf).unwrap();
        let (decoded, consumed) = amf0_decode(&buf).unwrap();
        assert_eq!(decoded, val);
        assert_eq!(consumed, buf.len());
    }

    #[test]
    fn amf0_object_roundtrip() {
        let val = Amf0Value::Object(vec![
            ("app".to_string(), Amf0Value::String("live".to_string())),
            ("version".to_string(), Amf0Value::Number(3.0)),
            ("flag".to_string(), Amf0Value::Boolean(true)),
        ]);
        let mut buf = Vec::new();
        amf0_encode(&val, &mut buf).unwrap();
        let (decoded, consumed) = amf0_decode(&buf).unwrap();
        assert_eq!(decoded, val);
        assert_eq!(consumed, buf.len());
    }

    // ── Chunk encoder ───────────────────────────────────────────────

    #[test]
    fn chunk_encoder_fmt0_basic() {
        let mut enc = ChunkEncoder::new();
        let msg = OutboundMessage {
            csid: 3,
            timestamp: 100,
            msg_type_id: MSG_COMMAND_AMF0,
            stream_id: 0,
            payload: vec![0xAA; 10],
        };
        let mut out = Vec::new();
        enc.encode_message(&msg, &mut out);

        // Basic header: 1 byte (fmt=0, csid=3).
        assert_eq!(out[0], 0x03); // fmt=0 (00) | csid=3 (000011)
                                  // Message header: 11 bytes.
                                  // Total header: 12 bytes + 10 payload = 22 bytes.
        assert_eq!(out.len(), 12 + 10);
    }

    #[test]
    fn chunk_encoder_splits_at_chunk_size() {
        let mut enc = ChunkEncoder::new();
        enc.set_chunk_size(10);
        let msg = OutboundMessage {
            csid: 3,
            timestamp: 0,
            msg_type_id: MSG_COMMAND_AMF0,
            stream_id: 0,
            payload: vec![0xBB; 25], // 3 chunks: 10 + 10 + 5
        };
        let mut out = Vec::new();
        enc.encode_message(&msg, &mut out);

        // First chunk: 12 (header) + 10 (data) = 22
        // Second chunk: 1 (fmt=3 header) + 10 (data) = 11
        // Third chunk: 1 (fmt=3 header) + 5 (data) = 6
        assert_eq!(out.len(), 22 + 11 + 6);
    }

    #[test]
    fn chunk_encoder_fmt_progression() {
        let mut enc = ChunkEncoder::new();
        let mut out = Vec::new();

        // First message: fmt=0 (12 bytes header).
        let msg1 = OutboundMessage {
            csid: 3,
            timestamp: 100,
            msg_type_id: MSG_AUDIO,
            stream_id: 1,
            payload: vec![0; 5],
        };
        enc.encode_message(&msg1, &mut out);
        assert_eq!(out[0] >> 6, 0); // fmt=0

        // Second message: same stream_id, different length → fmt=1.
        out.clear();
        let msg2 = OutboundMessage {
            csid: 3,
            timestamp: 120,
            msg_type_id: MSG_AUDIO,
            stream_id: 1,
            payload: vec![0; 10],
        };
        enc.encode_message(&msg2, &mut out);
        assert_eq!(out[0] >> 6, 1); // fmt=1

        // Third message: same length/type, different delta → fmt=2.
        out.clear();
        let msg3 = OutboundMessage {
            csid: 3,
            timestamp: 150,
            msg_type_id: MSG_AUDIO,
            stream_id: 1,
            payload: vec![0; 10],
        };
        enc.encode_message(&msg3, &mut out);
        assert_eq!(out[0] >> 6, 2); // fmt=2

        // Fourth message: same delta → fmt=3.
        out.clear();
        let msg4 = OutboundMessage {
            csid: 3,
            timestamp: 180,
            msg_type_id: MSG_AUDIO,
            stream_id: 1,
            payload: vec![0; 10],
        };
        enc.encode_message(&msg4, &mut out);
        assert_eq!(out[0] >> 6, 3); // fmt=3
    }

    #[test]
    fn chunk_encoder_extended_timestamp() {
        let mut enc = ChunkEncoder::new();
        let msg = OutboundMessage {
            csid: 3,
            timestamp: 0x01FF_FFFF, // > 0xFFFFFF
            msg_type_id: MSG_VIDEO,
            stream_id: 1,
            payload: vec![0; 5],
        };
        let mut out = Vec::new();
        enc.encode_message(&msg, &mut out);

        // Timestamp field in header should be 0xFFFFFF.
        assert_eq!(out[1], 0xFF);
        assert_eq!(out[2], 0xFF);
        assert_eq!(out[3], 0xFF);
        // Extended timestamp (4 bytes) follows the 11-byte message header.
        // Position 12..16 = extended timestamp.
        let ext = u32::from_be_bytes([out[12], out[13], out[14], out[15]]);
        assert_eq!(ext, 0x01FF_FFFF);
    }

    #[test]
    fn chunk_encoder_csid_assignment() {
        // Protocol control → csid=2.
        assert_eq!(CSID_PROTOCOL_CONTROL, 2);
        // Commands on stream 0 → csid=3.
        assert_eq!(csid_for_stream(0), 3);
        // Media on stream 1 → csid=4.
        assert_eq!(csid_for_stream(1), 4);
        // Media on stream 2 → csid=5.
        assert_eq!(csid_for_stream(2), 5);
    }

    // ── Chunk decoder ───────────────────────────────────────────────

    #[test]
    fn chunk_decode_fmt0_single_chunk() {
        // Encode a message, then decode it.
        let mut enc = ChunkEncoder::new();
        let msg = OutboundMessage {
            csid: 3,
            timestamp: 42,
            msg_type_id: MSG_COMMAND_AMF0,
            stream_id: 0,
            payload: vec![0x11, 0x22, 0x33],
        };
        let mut wire = Vec::new();
        enc.encode_message(&msg, &mut wire);

        let mut dec = ChunkDecoder::new();
        dec.push(&wire);
        let decoded = dec.decode_message().unwrap().unwrap();

        assert_eq!(decoded.timestamp, 42);
        assert_eq!(decoded.msg_type_id, MSG_COMMAND_AMF0);
        assert_eq!(decoded.stream_id, 0);
        assert_eq!(decoded.payload, vec![0x11, 0x22, 0x33]);
    }

    #[test]
    fn chunk_decode_multi_chunk_reassembly() {
        let mut enc = ChunkEncoder::new();
        enc.set_chunk_size(5);
        let payload = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
        let msg = OutboundMessage {
            csid: 3,
            timestamp: 0,
            msg_type_id: MSG_AUDIO,
            stream_id: 1,
            payload: payload.clone(),
        };
        let mut wire = Vec::new();
        enc.encode_message(&msg, &mut wire);

        let mut dec = ChunkDecoder::new();
        dec.set_chunk_size(5);
        dec.push(&wire);
        // Multi-chunk messages require multiple decode_message() calls —
        // each call processes one chunk and returns None until the final
        // chunk completes the message.
        let decoded = loop {
            if let Some(msg) = dec.decode_message().unwrap() {
                break msg;
            }
        };
        assert_eq!(decoded.payload, payload);
    }

    #[test]
    fn chunk_decode_partial_reads() {
        // Feed one byte at a time.
        let mut enc = ChunkEncoder::new();
        let msg = OutboundMessage {
            csid: 3,
            timestamp: 100,
            msg_type_id: MSG_VIDEO,
            stream_id: 1,
            payload: vec![0xAA, 0xBB, 0xCC],
        };
        let mut wire = Vec::new();
        enc.encode_message(&msg, &mut wire);

        let mut dec = ChunkDecoder::new();
        for (i, &byte) in wire.iter().enumerate() {
            dec.push(&[byte]);
            let result = dec.decode_message().unwrap();
            if i < wire.len() - 1 {
                assert!(result.is_none(), "Should not have a message yet at byte {i}");
            } else {
                let decoded = result.unwrap();
                assert_eq!(decoded.payload, vec![0xAA, 0xBB, 0xCC]);
            }
        }
    }

    // ── Handshake ───────────────────────────────────────────────────

    #[test]
    fn handshake_c0c1_length() {
        let (_, c0c1) = Handshake::new();
        assert_eq!(c0c1.len(), 1 + HANDSHAKE_SIZE);
        assert_eq!(c0c1[0], 0x03); // version
    }

    #[test]
    fn handshake_c1_not_all_zeros() {
        let (_, c0c1) = Handshake::new();
        // Random portion (bytes 9..1537) should not be all zeros.
        let random_portion = &c0c1[9..];
        assert!(random_portion.iter().any(|&b| b != 0), "C1 random data should not be all zeros");
    }

    #[test]
    fn handshake_full_flow() {
        let (mut hs, c0c1) = Handshake::new();

        // Simulate server sending S0+S1+S2.
        let mut server_response = Vec::new();
        server_response.push(0x03); // S0
        server_response.extend_from_slice(&vec![0xAA; HANDSHAKE_SIZE]); // S1
                                                                        // S2 = echo of C1.
        server_response.extend_from_slice(&c0c1[1..=HANDSHAKE_SIZE]); // S2

        let (c2, leftover) = hs.feed(&server_response).unwrap();
        assert_eq!(hs.state, HandshakeState::Complete);
        assert_eq!(c2, vec![0xAA; HANDSHAKE_SIZE]);
        assert!(leftover.is_empty());
    }

    #[test]
    fn handshake_incremental_feed() {
        let (mut hs, c0c1) = Handshake::new();

        let mut server_response = Vec::new();
        server_response.push(0x03);
        server_response.extend_from_slice(&vec![0xBB; HANDSHAKE_SIZE]);
        server_response.extend_from_slice(&c0c1[1..=HANDSHAKE_SIZE]);

        // Feed in small increments.
        let half = server_response.len() / 2;
        assert!(hs.feed(&server_response[..half]).is_none());
        assert_ne!(hs.state, HandshakeState::Complete);

        let (c2, leftover) = hs.feed(&server_response[half..]).unwrap();
        assert_eq!(hs.state, HandshakeState::Complete);
        assert_eq!(c2.len(), HANDSHAKE_SIZE);
        assert!(leftover.is_empty());
    }

    #[test]
    fn handshake_preserves_leftover_bytes() {
        // Simulate a server that pipelines S0+S1+S2 plus initial protocol
        // messages (e.g. WinAckSize) in the same TCP segment.  The leftover
        // bytes after the 3073-byte handshake must be returned so the caller
        // can forward them to the chunk decoder.
        let (mut hs, c0c1) = Handshake::new();

        let extra = b"\x02\x00\x00\x00\x00\x00\x04\x05\x00\x00\x00\x00\x00\x26\x25\xa0";

        let mut server_response = Vec::new();
        server_response.push(0x03); // S0
        server_response.extend_from_slice(&vec![0xCC; HANDSHAKE_SIZE]); // S1
        server_response.extend_from_slice(&c0c1[1..=HANDSHAKE_SIZE]); // S2
        server_response.extend_from_slice(extra); // extra post-handshake data

        let (c2, leftover) = hs.feed(&server_response).unwrap();
        assert_eq!(hs.state, HandshakeState::Complete);
        assert_eq!(c2, vec![0xCC; HANDSHAKE_SIZE]);
        assert_eq!(leftover, extra);
    }

    // ── AvcSequenceHeader ───────────────────────────────────────────

    #[test]
    fn avc_sequence_header_to_bytes() {
        let header = AvcSequenceHeader {
            avc_profile_indication: 0x42,
            profile_compatibility: 0xC0,
            avc_level_indication: 0x1F,
            length_size_minus_one: 3,
            sps_list: vec![vec![0x67, 0x42, 0xC0, 0x1F]],
            pps_list: vec![vec![0x68, 0xCE, 0x38, 0x80]],
        };
        let bytes = header.to_bytes().unwrap();

        assert_eq!(bytes[0], 1); // configurationVersion
        assert_eq!(bytes[1], 0x42); // profile
        assert_eq!(bytes[2], 0xC0); // compatibility
        assert_eq!(bytes[3], 0x1F); // level
        assert_eq!(bytes[4], 0xFF); // 111111 | 11 (length_size_minus_one=3)
        assert_eq!(bytes[5] & 0x1F, 1); // numSPS = 1
                                        // SPS length (2 bytes) + SPS data (4 bytes).
        assert_eq!(bytes[6], 0);
        assert_eq!(bytes[7], 4);
        assert_eq!(&bytes[8..12], &[0x67, 0x42, 0xC0, 0x1F]);
        // numPPS = 1
        assert_eq!(bytes[12], 1);
        // PPS length (2 bytes) + PPS data (4 bytes).
        assert_eq!(bytes[13], 0);
        assert_eq!(bytes[14], 4);
        assert_eq!(&bytes[15..19], &[0x68, 0xCE, 0x38, 0x80]);
    }

    #[test]
    fn avc_sequence_header_no_sps_errors() {
        let header = AvcSequenceHeader {
            avc_profile_indication: 0x42,
            profile_compatibility: 0xC0,
            avc_level_indication: 0x1F,
            length_size_minus_one: 3,
            sps_list: vec![],
            pps_list: vec![vec![0x68]],
        };
        assert!(header.to_bytes().is_err());
    }

    // ── FLV header byte construction ────────────────────────────────

    #[test]
    fn flv_video_header_keyframe_avc() {
        // KeyFrame (1) << 4 | AVC (7) = 0x17.
        let frame_type: u8 = 1;
        let codec: u8 = 7;
        assert_eq!((frame_type << 4) | codec, 0x17);
    }

    #[test]
    fn flv_video_header_interframe_avc() {
        // InterFrame (2) << 4 | AVC (7) = 0x27.
        let frame_type: u8 = 2;
        let codec: u8 = 7;
        assert_eq!((frame_type << 4) | codec, 0x27);
    }

    #[test]
    fn flv_audio_header_aac() {
        // AAC (10) << 4 | 44kHz (3) << 2 | 16bit (1) << 1 | stereo (1) = 0xAF.
        let format: u8 = 10;
        let rate: u8 = 3;
        let size: u8 = 1;
        let channels: u8 = 1;
        assert_eq!((format << 4) | (rate << 2) | (size << 1) | channels, 0xAF);
    }

    // ── State machine ───────────────────────────────────────────────

    #[test]
    fn connection_starts_in_handshaking() {
        let url = RtmpUrl::parse("rtmp://127.0.0.1/live/key").unwrap();
        let conn = RtmpPublishClientConnection::new(url);
        assert_eq!(conn.state(), RtmpConnectionState::Handshaking);
    }

    #[test]
    fn connection_c0c1_in_send_buf() {
        let url = RtmpUrl::parse("rtmp://127.0.0.1/live/key").unwrap();
        let conn = RtmpPublishClientConnection::new(url);
        let buf = conn.send_buf();
        assert_eq!(buf.len(), 1 + HANDSHAKE_SIZE);
        assert_eq!(buf[0], 0x03);
    }

    #[test]
    fn connection_advance_send_buf() {
        let url = RtmpUrl::parse("rtmp://127.0.0.1/live/key").unwrap();
        let mut conn = RtmpPublishClientConnection::new(url);
        let initial_len = conn.send_buf().len();
        conn.advance_send_buf(10);
        assert_eq!(conn.send_buf().len(), initial_len - 10);
    }

    #[test]
    fn connection_handshake_transitions_to_connecting() {
        let url = RtmpUrl::parse("rtmp://127.0.0.1/live/key").unwrap();
        let mut conn = RtmpPublishClientConnection::new(url);

        // Get C0+C1 from send buf.
        let c0c1 = conn.send_buf().to_vec();

        // Simulate S0+S1+S2.
        let mut server = Vec::new();
        server.push(0x03); // S0
        server.extend_from_slice(&vec![0xCC; HANDSHAKE_SIZE]); // S1
        server.extend_from_slice(&c0c1[1..=HANDSHAKE_SIZE]); // S2 = echo C1

        conn.feed_recv_buf(&server).unwrap();
        assert_eq!(conn.state(), RtmpConnectionState::Connecting);

        // Send buf should have: C0+C1 + C2 + WinAckSize + SetChunkSize + connect
        assert!(conn.send_buf().len() > 1 + HANDSHAKE_SIZE);
    }

    #[test]
    fn connection_send_video_before_publishing_errors() {
        let url = RtmpUrl::parse("rtmp://127.0.0.1/live/key").unwrap();
        let mut conn = RtmpPublishClientConnection::new(url);
        let frame = VideoFrame {
            timestamp: RtmpTimestamp::from_millis(0),
            composition_timestamp_offset: RtmpTimestampDelta::ZERO,
            frame_type: VideoFrameType::KeyFrame,
            codec: VideoCodec::Avc,
            avc_packet_type: Some(AvcPacketType::NalUnit),
            data: vec![0; 10],
        };
        assert!(conn.send_video(&frame).is_err());
    }

    #[test]
    fn connection_display_impl() {
        let state = RtmpConnectionState::Publishing;
        assert_eq!(format!("{state}"), "Publishing");
    }

    // ── Encode/decode roundtrip ─────────────────────────────────────

    #[test]
    fn encode_decode_roundtrip_various_messages() {
        let messages = vec![
            OutboundMessage {
                csid: 2,
                timestamp: 0,
                msg_type_id: MSG_WIN_ACK_SIZE,
                stream_id: 0,
                payload: 2_500_000u32.to_be_bytes().to_vec(),
            },
            OutboundMessage {
                csid: 3,
                timestamp: 100,
                msg_type_id: MSG_COMMAND_AMF0,
                stream_id: 0,
                payload: vec![0x02, 0x00, 0x07, b'c', b'o', b'n', b'n', b'e', b'c', b't'],
            },
            OutboundMessage {
                csid: 4,
                timestamp: 1000,
                msg_type_id: MSG_VIDEO,
                stream_id: 1,
                payload: vec![0x17, 0x00, 0x00, 0x00, 0x00, 0xAA, 0xBB],
            },
        ];

        for orig in &messages {
            let mut enc = ChunkEncoder::new();
            let mut wire = Vec::new();
            enc.encode_message(orig, &mut wire);

            let mut dec = ChunkDecoder::new();
            dec.push(&wire);
            let decoded = dec.decode_message().unwrap().unwrap();

            assert_eq!(
                decoded.timestamp, orig.timestamp,
                "timestamp mismatch for csid={}",
                orig.csid
            );
            assert_eq!(
                decoded.msg_type_id, orig.msg_type_id,
                "type mismatch for csid={}",
                orig.csid
            );
            assert_eq!(
                decoded.stream_id, orig.stream_id,
                "stream_id mismatch for csid={}",
                orig.csid
            );
            assert_eq!(decoded.payload, orig.payload, "payload mismatch for csid={}", orig.csid);
        }
    }

    // ── Basic header encoding ───────────────────────────────────────

    #[test]
    fn basic_header_1byte_form() {
        let mut out = Vec::new();
        encode_basic_header(0, 2, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0], 0x02); // fmt=0, csid=2
    }

    #[test]
    fn basic_header_2byte_form() {
        let mut out = Vec::new();
        encode_basic_header(0, 64, &mut out);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], 0x00); // fmt=0, csid=0 (2-byte marker)
        assert_eq!(out[1], 0); // 64 - 64 = 0
    }

    #[test]
    fn basic_header_3byte_form() {
        let mut out = Vec::new();
        encode_basic_header(0, 320, &mut out);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0], 0x01); // fmt=0, csid=1 (3-byte marker)
        let val = u16::from(out[1]) + u16::from(out[2]) * 256 + 64;
        assert_eq!(val, 320);
    }
}
