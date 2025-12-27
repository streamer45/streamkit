// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Type conversions between C ABI types and Rust types
//!
//! These functions provide safe wrappers around unsafe FFI operations.

use crate::types::{
    CAudioFormat, CAudioFrame, CCustomEncoding, CCustomPacket, CPacket, CPacketMetadata,
    CPacketType, CPacketTypeInfo, CSampleFormat,
};
use std::cell::RefCell;
use std::ffi::{c_void, CStr, CString};
use std::os::raw::c_char;
use std::sync::Arc;
use streamkit_core::types::{
    AudioFormat, AudioFrame, CustomEncoding, CustomPacketData, Packet, PacketMetadata, PacketType,
    SampleFormat, TranscriptionData,
};

/// Convert C packet type info to Rust PacketType
///
/// # Errors
///
/// Returns an error if:
/// - `RawAudio` is missing its `audio_format`
/// - `Custom` is missing its `custom_type_id`
/// - `custom_type_id` is not valid UTF-8
pub fn packet_type_from_c(cpt_info: CPacketTypeInfo) -> Result<PacketType, String> {
    match cpt_info.type_discriminant {
        CPacketType::RawAudio => {
            if cpt_info.audio_format.is_null() {
                return Err("RawAudio packet type missing audio_format".to_string());
            }
            // SAFETY: caller guarantees pointer validity for the duration of this call.
            let c_format = unsafe { &*cpt_info.audio_format };
            Ok(PacketType::RawAudio(audio_format_from_c(c_format)))
        },
        CPacketType::OpusAudio => Ok(PacketType::OpusAudio),
        CPacketType::Text => Ok(PacketType::Text),
        CPacketType::Transcription => Ok(PacketType::Transcription),
        CPacketType::Custom => {
            if cpt_info.custom_type_id.is_null() {
                return Err("Custom packet type missing custom_type_id".to_string());
            }
            let type_id = unsafe { c_str_to_string(cpt_info.custom_type_id) }?;
            Ok(PacketType::Custom { type_id })
        },
        CPacketType::Binary => Ok(PacketType::Binary),
        CPacketType::Any => Ok(PacketType::Any),
        CPacketType::Passthrough => Ok(PacketType::Passthrough),
    }
}

/// Convert Rust SampleFormat to C
pub const fn sample_format_to_c(sf: &SampleFormat) -> CSampleFormat {
    match sf {
        SampleFormat::F32 => CSampleFormat::F32,
        SampleFormat::S16Le => CSampleFormat::S16Le,
    }
}

/// Convert C sample format to Rust
pub const fn sample_format_from_c(csf: CSampleFormat) -> SampleFormat {
    match csf {
        CSampleFormat::F32 => SampleFormat::F32,
        CSampleFormat::S16Le => SampleFormat::S16Le,
    }
}

/// Convert Rust AudioFormat to C
pub const fn audio_format_to_c(af: &AudioFormat) -> CAudioFormat {
    CAudioFormat {
        sample_rate: af.sample_rate,
        channels: af.channels,
        sample_format: sample_format_to_c(&af.sample_format),
    }
}

/// Convert C AudioFormat to Rust
pub const fn audio_format_from_c(caf: &CAudioFormat) -> AudioFormat {
    AudioFormat {
        sample_rate: caf.sample_rate,
        channels: caf.channels,
        sample_format: sample_format_from_c(caf.sample_format),
    }
}

/// Convert Rust PacketType to C representation
/// Returns (CPacketTypeInfo, optional CAudioFormat that must be kept alive)
/// For RawAudio types, the returned CAudioFormat must outlive the CPacketTypeInfo
pub const fn packet_type_to_c(pt: &PacketType) -> (CPacketTypeInfo, Option<CAudioFormat>) {
    match pt {
        PacketType::RawAudio(format) => {
            let c_format = audio_format_to_c(format);
            (
                CPacketTypeInfo {
                    type_discriminant: CPacketType::RawAudio,
                    audio_format: &raw const c_format,
                    custom_type_id: std::ptr::null(),
                },
                Some(c_format),
            )
        },
        PacketType::OpusAudio => (
            CPacketTypeInfo {
                type_discriminant: CPacketType::OpusAudio,
                audio_format: std::ptr::null(),
                custom_type_id: std::ptr::null(),
            },
            None,
        ),
        PacketType::Text => (
            CPacketTypeInfo {
                type_discriminant: CPacketType::Text,
                audio_format: std::ptr::null(),
                custom_type_id: std::ptr::null(),
            },
            None,
        ),
        PacketType::Transcription => (
            CPacketTypeInfo {
                type_discriminant: CPacketType::Transcription,
                audio_format: std::ptr::null(),
                custom_type_id: std::ptr::null(),
            },
            None,
        ),
        PacketType::Custom { .. } => (
            CPacketTypeInfo {
                type_discriminant: CPacketType::Custom,
                audio_format: std::ptr::null(),
                custom_type_id: std::ptr::null(), // provided by the caller where stable storage exists
            },
            None,
        ),
        PacketType::Binary => (
            CPacketTypeInfo {
                type_discriminant: CPacketType::Binary,
                audio_format: std::ptr::null(),
                custom_type_id: std::ptr::null(),
            },
            None,
        ),
        PacketType::Any => (
            CPacketTypeInfo {
                type_discriminant: CPacketType::Any,
                audio_format: std::ptr::null(),
                custom_type_id: std::ptr::null(),
            },
            None,
        ),
        PacketType::Passthrough => (
            CPacketTypeInfo {
                type_discriminant: CPacketType::Passthrough,
                audio_format: std::ptr::null(),
                custom_type_id: std::ptr::null(),
            },
            None,
        ),
    }
}

pub struct CPacketRepr {
    pub packet: CPacket,
    _owned: CPacketOwned,
}

#[allow(dead_code)] // Owned values are kept alive to support FFI pointers during callbacks.
enum CPacketOwned {
    None,
    Audio(Box<CAudioFrame>),
    Text(CString),
    Bytes(Vec<u8>),
    Custom(CustomOwned),
}

#[allow(dead_code)] // Owned values are kept alive to support FFI pointers during callbacks.
struct CustomOwned {
    type_id: CString,
    data_json: Vec<u8>,
    metadata: Option<Box<CPacketMetadata>>,
    custom: Box<CCustomPacket>,
}

fn metadata_to_c(meta: &PacketMetadata) -> CPacketMetadata {
    CPacketMetadata {
        timestamp_us: meta.timestamp_us.unwrap_or_default(),
        has_timestamp_us: meta.timestamp_us.is_some(),
        duration_us: meta.duration_us.unwrap_or_default(),
        has_duration_us: meta.duration_us.is_some(),
        sequence: meta.sequence.unwrap_or_default(),
        has_sequence: meta.sequence.is_some(),
    }
}

fn metadata_from_c(meta: &CPacketMetadata) -> PacketMetadata {
    PacketMetadata {
        timestamp_us: meta.has_timestamp_us.then_some(meta.timestamp_us),
        duration_us: meta.has_duration_us.then_some(meta.duration_us),
        sequence: meta.has_sequence.then_some(meta.sequence),
    }
}

fn cstring_sanitize(s: &str) -> CString {
    CString::new(s).unwrap_or_else(|_| CString::new(s.replace('\0', " ")).unwrap_or_default())
}

/// Convert Rust Packet to C representation.
///
/// The returned representation owns any allocations needed for the duration of the C callback.
pub fn packet_to_c(packet: &Packet) -> CPacketRepr {
    match packet {
        Packet::Audio(frame) => {
            let c_frame = Box::new(CAudioFrame {
                sample_rate: frame.sample_rate,
                channels: frame.channels,
                samples: frame.samples.as_ptr(),
                sample_count: frame.samples.len(),
            });
            let packet = CPacket {
                packet_type: CPacketType::RawAudio,
                data: std::ptr::from_ref::<CAudioFrame>(&*c_frame).cast::<c_void>(),
                len: std::mem::size_of::<CAudioFrame>(),
            };
            CPacketRepr { packet, _owned: CPacketOwned::Audio(c_frame) }
        },
        Packet::Text(text) => {
            let s = text.as_ref();
            let c_str = match CString::new(s) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(
                        "Text packet contains null bytes (position {}), data will be truncated",
                        e.nul_position()
                    );
                    let truncated = &s[..e.nul_position()];
                    CString::new(truncated).unwrap_or_default()
                },
            };
            let packet = CPacket {
                packet_type: CPacketType::Text,
                data: c_str.as_ptr().cast::<c_void>(),
                len: c_str.as_bytes_with_nul().len(),
            };
            CPacketRepr { packet, _owned: CPacketOwned::Text(c_str) }
        },
        Packet::Transcription(trans_data) => {
            let json = serde_json::to_vec(trans_data).unwrap_or_else(|e| {
                tracing::error!("Failed to serialize transcription data to JSON: {}", e);
                b"{}".to_vec()
            });
            let packet = CPacket {
                packet_type: CPacketType::Transcription,
                data: json.as_ptr().cast::<c_void>(),
                len: json.len(),
            };
            CPacketRepr { packet, _owned: CPacketOwned::Bytes(json) }
        },
        Packet::Custom(custom) => {
            let type_id = cstring_sanitize(custom.type_id.as_str());
            let data_json = serde_json::to_vec(&custom.data).unwrap_or_else(|e| {
                tracing::error!("Failed to serialize custom packet data to JSON: {}", e);
                b"{}".to_vec()
            });

            let metadata = custom.metadata.as_ref().map(|m| Box::new(metadata_to_c(m)));
            let mut custom_packet = Box::new(CCustomPacket {
                type_id: type_id.as_ptr(),
                encoding: match custom.encoding {
                    CustomEncoding::Json => CCustomEncoding::Json,
                },
                data_json: data_json.as_ptr(),
                data_len: data_json.len(),
                metadata: metadata.as_deref().map_or(std::ptr::null(), std::ptr::from_ref),
            });

            let packet = CPacket {
                packet_type: CPacketType::Custom,
                data: std::ptr::from_mut::<CCustomPacket>(&mut *custom_packet).cast::<c_void>(),
                len: std::mem::size_of::<CCustomPacket>(),
            };

            CPacketRepr {
                packet,
                _owned: CPacketOwned::Custom(CustomOwned {
                    type_id,
                    data_json,
                    metadata,
                    custom: custom_packet,
                }),
            }
        },
        Packet::Binary { data, .. } => CPacketRepr {
            packet: CPacket {
                packet_type: CPacketType::Binary,
                data: data.as_ref().as_ptr().cast::<c_void>(),
                len: data.len(),
            },
            _owned: CPacketOwned::None,
        },
    }
}

/// Convert C packet to Rust Packet
///
/// # Safety
///
/// The caller must ensure:
/// - The CPacket pointer is valid
/// - The data pointer is valid and points to data of the specified length
/// - The data remains valid for the duration of this call
///
/// # Errors
///
/// Returns an error if:
/// - The packet pointer is null
/// - The data pointer is null
/// - The packet type is unsupported
/// - The packet data is invalid (e.g., invalid UTF-8, malformed JSON)
pub unsafe fn packet_from_c(c_packet: *const CPacket) -> Result<Packet, String> {
    if c_packet.is_null() {
        return Err("Null packet pointer".to_string());
    }

    let c_pkt = &*c_packet;

    if c_pkt.data.is_null() {
        return Err("Null packet data pointer".to_string());
    }

    match c_pkt.packet_type {
        CPacketType::RawAudio => {
            let c_frame = &*c_pkt.data.cast::<CAudioFrame>();
            if c_frame.samples.is_null() {
                return Err("Null samples pointer in audio frame".to_string());
            }

            let samples = std::slice::from_raw_parts(c_frame.samples, c_frame.sample_count);

            Ok(Packet::Audio(AudioFrame::new(
                c_frame.sample_rate,
                c_frame.channels,
                samples.to_vec(),
            )))
        },
        CPacketType::Text => {
            let c_str = CStr::from_ptr(c_pkt.data.cast::<c_char>());
            let text = c_str
                .to_str()
                .map_err(|e| format!("Invalid UTF-8 in text packet: {e}"))?
                .to_string();
            Ok(Packet::Text(text.into()))
        },
        CPacketType::Transcription => {
            // Deserialize JSON transcription data
            let data = std::slice::from_raw_parts(c_pkt.data.cast::<u8>(), c_pkt.len);
            let trans_data: TranscriptionData = serde_json::from_slice(data)
                .map_err(|e| format!("Invalid transcription data: {e}"))?;
            Ok(Packet::Transcription(Arc::new(trans_data)))
        },
        CPacketType::Custom => {
            let c_custom = &*c_pkt.data.cast::<CCustomPacket>();
            if c_custom.type_id.is_null() {
                return Err("Custom packet missing type_id".to_string());
            }
            if c_custom.data_json.is_null() {
                return Err("Custom packet missing data_json".to_string());
            }

            let type_id = c_str_to_string(c_custom.type_id)?;
            let data_bytes = std::slice::from_raw_parts(c_custom.data_json, c_custom.data_len);
            let data: serde_json::Value = serde_json::from_slice(data_bytes)
                .map_err(|e| format!("Invalid custom JSON: {e}"))?;

            let metadata = if c_custom.metadata.is_null() {
                None
            } else {
                Some(metadata_from_c(&*c_custom.metadata))
            };

            let encoding = match c_custom.encoding {
                CCustomEncoding::Json => CustomEncoding::Json,
            };

            Ok(Packet::Custom(Arc::new(CustomPacketData { type_id, encoding, data, metadata })))
        },
        CPacketType::Binary => {
            let data = std::slice::from_raw_parts(c_pkt.data.cast::<u8>(), c_pkt.len);
            Ok(Packet::Binary {
                data: bytes::Bytes::copy_from_slice(data),
                content_type: None,
                metadata: None,
            })
        },
        _ => Err(format!("Unsupported packet type: {:?}", c_pkt.packet_type)),
    }
}

/// Convert C string to Rust String
///
/// # Safety
///
/// The pointer must be a valid null-terminated C string
///
/// # Errors
///
/// Returns an error if the string contains invalid UTF-8
pub unsafe fn c_str_to_string(ptr: *const c_char) -> Result<String, String> {
    if ptr.is_null() {
        return Ok(String::new());
    }

    CStr::from_ptr(ptr)
        .to_str()
        .map(std::string::ToString::to_string)
        .map_err(|e| format!("Invalid UTF-8: {e}"))
}

/// Convert Rust string to C string (caller must free)
///
/// # Panics
///
/// Panics if the string contains null bytes
#[allow(clippy::expect_used)] // expect is appropriate here - null bytes in strings are programmer errors
pub fn string_to_c(s: &str) -> *const c_char {
    CString::new(s).expect("String should not contain null bytes").into_raw()
}

/// Convert an error message to a C string for returning across the C ABI.
///
/// # Ownership and lifetime
///
/// The returned pointer is **borrowed** and **must not be freed** by the caller.
/// It remains valid until the next `error_to_c()` call on the same OS thread.
///
/// This design:
/// - Prevents host-side leaks when the host copies the message into an owned string.
/// - Avoids cross-dylib allocator issues (freeing memory in a different module).
pub fn error_to_c(msg: impl AsRef<str>) -> *const c_char {
    thread_local! {
        static LAST_ERROR: RefCell<CString> = RefCell::new(
            // Empty string; always a valid null-terminated C string.
            CString::new("").unwrap_or_else(|_| unsafe { CString::from_vec_unchecked(vec![0]) })
        );
    }

    let msg = msg.as_ref();
    let sanitized = if msg.contains('\0') { msg.replace('\0', " ") } else { msg.to_string() };

    // CString::new can only fail if there are interior null bytes. We sanitize them above,
    // but avoid panicking at this FFI boundary and fall back to an empty string if needed.
    let c_str =
        CString::new(sanitized).unwrap_or_else(|_| unsafe { CString::from_vec_unchecked(vec![0]) });

    LAST_ERROR.with(|slot| {
        *slot.borrow_mut() = c_str;
        slot.borrow().as_ptr()
    })
}

/// Free a C string created by [`string_to_c`].
/// # Safety
/// The pointer must have been created by `string_to_c` and not freed yet.
pub unsafe fn free_c_string(ptr: *const c_char) {
    if !ptr.is_null() {
        drop(CString::from_raw(ptr.cast_mut()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_to_c_normal_string() {
        let msg = "Test error message";
        let c_msg = error_to_c(msg);
        unsafe {
            let result_cstr = CStr::from_ptr(c_msg);
            assert_eq!(result_cstr.to_string_lossy(), msg);
        }
    }

    #[test]
    fn test_error_to_c_with_null_bytes() {
        let msg = "Error\0with\0null\0bytes";
        let c_msg = error_to_c(msg);
        unsafe {
            let result_cstr = CStr::from_ptr(c_msg);
            let result = result_cstr.to_string_lossy();
            // Null bytes should be replaced with spaces
            assert_eq!(result, "Error with null bytes");
        }
    }

    #[test]
    fn test_error_to_c_format_string() {
        let msg = format!("Error code: {}", 42);
        let c_msg = error_to_c(&msg);
        unsafe {
            let result_cstr = CStr::from_ptr(c_msg);
            assert_eq!(result_cstr.to_string_lossy(), "Error code: 42");
        }
    }

    #[test]
    fn test_string_to_c_requires_free() {
        let c_msg = string_to_c("hello");
        unsafe {
            let result_cstr = CStr::from_ptr(c_msg);
            assert_eq!(result_cstr.to_string_lossy(), "hello");
            free_c_string(c_msg);
        }
    }
}
