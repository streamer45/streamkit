// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Internal backing storage for plugin metadata.
//!
//! [`PluginMetadataStorage`] owns all C-string and C-struct allocations
//! needed by the [`CNodeMetadata`] pointer returned from
//! `__plugin_get_metadata`.  It lives in a `static OnceLock` inside the
//! plugin's dylib — initialized once, never moved.

use std::ffi::CString;
use std::os::raw::c_char;

use crate::conversions;
use crate::ffi_guard;
use crate::types::{
    CAudioFormat, CInputPin, CNodeMetadata, COutputPin, CPacketType, CPacketTypeInfo,
    CRawVideoFormat,
};
use crate::NodeMetadata;

/// Owns all heap allocations backing a [`CNodeMetadata`].
///
/// Raw pointers inside `c_metadata`, `inputs`, and `outputs` point into
/// the sibling `Vec`/`CString` fields.  Because the struct lives in a
/// `static OnceLock` and is never moved after initialization, the
/// pointers remain valid for the lifetime of the plugin.
pub struct PluginMetadataStorage {
    pub c_metadata: CNodeMetadata,
    pub inputs: Vec<CInputPin>,
    pub input_names: Vec<CString>,
    pub input_types: Vec<Vec<CPacketTypeInfo>>,
    pub input_audio_formats: Vec<Vec<Option<CAudioFormat>>>,
    pub input_custom_type_ids: Vec<Vec<Option<CString>>>,
    pub input_video_formats: Vec<Vec<Option<CRawVideoFormat>>>,
    pub outputs: Vec<COutputPin>,
    pub output_names: Vec<CString>,
    pub output_audio_formats: Vec<Option<CAudioFormat>>,
    pub output_custom_type_ids: Vec<Option<CString>>,
    pub output_video_formats: Vec<Option<CRawVideoFormat>>,
    pub category_strings: Vec<CString>,
    pub category_ptrs: Vec<*const c_char>,
    pub kind: CString,
    pub description: Option<CString>,
    pub param_schema: CString,
}

// SAFETY: All raw pointers in PluginMetadataStorage point to owned data
// within the same struct instance. The struct is stored in a static OnceLock
// and never moved after initialization.
#[allow(clippy::non_send_fields_in_send_ty)]
unsafe impl Send for PluginMetadataStorage {}
unsafe impl Sync for PluginMetadataStorage {}

impl PluginMetadataStorage {
    /// Build storage from a [`NodeMetadata`] value.
    ///
    /// This extracts the ~200 lines of metadata conversion that was
    /// previously duplicated between `native_plugin_entry!` and
    /// `native_source_plugin_entry!`.
    ///
    /// # Panics
    ///
    /// Cannot panic in practice — the `unwrap()` calls on `last()` are
    /// reached only immediately after pushing an element to the same `Vec`.
    pub fn from_node_metadata(meta: &NodeMetadata) -> Self {
        // Invariant: the raw pointers stored in `c_inputs`/`c_outputs`
        // are captured via `.as_ptr()` on Vec elements immediately after
        // pushing.  These pointers remain valid because:
        //   1. Each Vec is only ever pushed to (never popped or cleared).
        //   2. The Vecs are moved into the returned struct and never
        //      reallocated after that.
        //   3. The struct lives in a `OnceLock` and is never moved again.
        // A future refactor that calls `.to_vec()`, sorts, or otherwise
        // reallocates a Vec would invalidate these pointers — use this
        // comment as a guard rail.
        //
        // ── Convert inputs ──────────────────────────────────────────
        let mut c_inputs = Vec::new();
        let mut input_names = Vec::new();
        let mut input_types = Vec::new();
        let mut input_audio_formats = Vec::new();
        let mut input_custom_type_ids = Vec::new();
        let mut input_video_formats = Vec::new();

        for input in &meta.inputs {
            let name = ffi_guard::cstring_lossy(input.name.as_str(), "input pin name");
            let mut types_info = Vec::new();
            let mut audio_formats = Vec::new();
            let mut custom_type_ids = Vec::new();
            let mut video_formats = Vec::new();

            for pt in &input.accepts_types {
                let audio_format = match pt {
                    streamkit_core::types::PacketType::RawAudio(af) => {
                        Some(conversions::audio_format_to_c(af))
                    },
                    _ => None,
                };
                audio_formats.push(audio_format);

                let custom_type_id = match pt {
                    streamkit_core::types::PacketType::Custom { type_id } => {
                        Some(ffi_guard::cstring_lossy(type_id.as_str(), "custom type_id"))
                    },
                    streamkit_core::types::PacketType::EncodedAudio(format)
                        if format.codec == streamkit_core::types::AudioCodec::Opus
                            && format.codec_private.is_none() =>
                    {
                        None
                    },
                    streamkit_core::types::PacketType::EncodedAudio(format) => {
                        Some(conversions::codec_name_to_cstring(format.codec.as_c_name()))
                    },
                    streamkit_core::types::PacketType::EncodedVideo(format) => {
                        Some(conversions::codec_name_to_cstring(format.codec.as_c_name()))
                    },
                    _ => None,
                };
                custom_type_ids.push(custom_type_id);

                let video_format = match pt {
                    streamkit_core::types::PacketType::RawVideo(vf) => {
                        Some(conversions::raw_video_format_to_c(vf))
                    },
                    _ => None,
                };
                video_formats.push(video_format);
            }

            for (idx, pt) in input.accepts_types.iter().enumerate() {
                let type_discriminant = packet_type_to_c_discriminant(pt);

                let audio_format_ptr =
                    audio_formats[idx].as_ref().map_or(std::ptr::null(), std::ptr::from_ref);

                let custom_type_id_ptr =
                    custom_type_ids[idx].as_ref().map_or(std::ptr::null(), |s| s.as_ptr());

                let video_format_ptr =
                    video_formats[idx].as_ref().map_or(std::ptr::null(), std::ptr::from_ref);

                types_info.push(CPacketTypeInfo {
                    type_discriminant,
                    audio_format: audio_format_ptr,
                    custom_type_id: custom_type_id_ptr,
                    raw_video_format: video_format_ptr,
                });
            }

            c_inputs.push(CInputPin {
                name: name.as_ptr(),
                accepts_types: types_info.as_ptr(),
                accepts_types_count: types_info.len(),
            });

            input_names.push(name);
            input_types.push(types_info);
            input_audio_formats.push(audio_formats);
            input_custom_type_ids.push(custom_type_ids);
            input_video_formats.push(video_formats);
        }

        // ── Convert outputs ─────────────────────────────────────────
        let mut c_outputs = Vec::with_capacity(meta.outputs.len());
        let mut output_names = Vec::with_capacity(meta.outputs.len());
        let mut output_audio_formats: Vec<Option<CAudioFormat>> =
            Vec::with_capacity(meta.outputs.len());
        let mut output_custom_type_ids: Vec<Option<CString>> =
            Vec::with_capacity(meta.outputs.len());
        let mut output_video_formats: Vec<Option<CRawVideoFormat>> =
            Vec::with_capacity(meta.outputs.len());

        for output in &meta.outputs {
            output_names.push(ffi_guard::cstring_lossy(output.name.as_str(), "output pin name"));
            output_audio_formats.push(match &output.produces_type {
                streamkit_core::types::PacketType::RawAudio(af) => {
                    Some(conversions::audio_format_to_c(af))
                },
                _ => None,
            });
            output_custom_type_ids.push(match &output.produces_type {
                streamkit_core::types::PacketType::Custom { type_id } => {
                    Some(ffi_guard::cstring_lossy(type_id.as_str(), "output custom type_id"))
                },
                streamkit_core::types::PacketType::EncodedAudio(format)
                    if format.codec == streamkit_core::types::AudioCodec::Opus
                        && format.codec_private.is_none() =>
                {
                    None
                },
                streamkit_core::types::PacketType::EncodedAudio(format) => {
                    Some(conversions::codec_name_to_cstring(format.codec.as_c_name()))
                },
                streamkit_core::types::PacketType::EncodedVideo(format) => {
                    Some(conversions::codec_name_to_cstring(format.codec.as_c_name()))
                },
                _ => None,
            });
            output_video_formats.push(match &output.produces_type {
                streamkit_core::types::PacketType::RawVideo(vf) => {
                    Some(conversions::raw_video_format_to_c(vf))
                },
                _ => None,
            });
        }

        for (idx, output) in meta.outputs.iter().enumerate() {
            let type_discriminant = packet_type_to_c_discriminant(&output.produces_type);

            let audio_format_ptr = output_audio_formats
                .get(idx)
                .and_then(Option::as_ref)
                .map_or(std::ptr::null(), std::ptr::from_ref);

            let custom_type_id_ptr = output_custom_type_ids
                .get(idx)
                .and_then(Option::as_ref)
                .map_or(std::ptr::null(), |s| s.as_ptr());

            let video_format_ptr = output_video_formats
                .get(idx)
                .and_then(Option::as_ref)
                .map_or(std::ptr::null(), std::ptr::from_ref);

            let type_info = CPacketTypeInfo {
                type_discriminant,
                audio_format: audio_format_ptr,
                custom_type_id: custom_type_id_ptr,
                raw_video_format: video_format_ptr,
            };

            let name = output_names.get(idx).as_ref().map_or(std::ptr::null(), |s| s.as_ptr());

            c_outputs.push(COutputPin { name, produces_type: type_info });
        }

        // ── Convert categories ──────────────────────────────────────
        let mut category_strings = Vec::new();
        let mut category_ptrs = Vec::new();

        for cat in &meta.categories {
            let c_str = ffi_guard::cstring_lossy(cat.as_str(), "category name");
            category_ptrs.push(c_str.as_ptr());
            category_strings.push(c_str);
        }

        // ── Scalar fields ───────────────────────────────────────────
        let kind = ffi_guard::cstring_lossy(meta.kind.as_str(), "node kind");
        let description =
            meta.description.as_ref().map(|d| ffi_guard::cstring_lossy(d.as_str(), "description"));
        let param_schema =
            ffi_guard::cstring_lossy(&meta.param_schema.to_string(), "param schema JSON");

        let c_metadata = CNodeMetadata {
            kind: kind.as_ptr(),
            description: description.as_ref().map_or(std::ptr::null(), |d| d.as_ptr()),
            inputs: c_inputs.as_ptr(),
            inputs_count: c_inputs.len(),
            outputs: c_outputs.as_ptr(),
            outputs_count: c_outputs.len(),
            param_schema: param_schema.as_ptr(),
            categories: category_ptrs.as_ptr(),
            categories_count: category_ptrs.len(),
        };

        Self {
            c_metadata,
            inputs: c_inputs,
            input_names,
            input_types,
            input_audio_formats,
            input_custom_type_ids,
            input_video_formats,
            outputs: c_outputs,
            output_names,
            output_audio_formats,
            output_custom_type_ids,
            output_video_formats,
            category_strings,
            category_ptrs,
            kind,
            description,
            param_schema,
        }
    }
}

/// Map a Rust [`PacketType`] to the corresponding [`CPacketType`] discriminant.
fn packet_type_to_c_discriminant(pt: &streamkit_core::types::PacketType) -> CPacketType {
    match pt {
        streamkit_core::types::PacketType::RawAudio(_) => CPacketType::RawAudio,
        streamkit_core::types::PacketType::EncodedAudio(format) => {
            if format.codec == streamkit_core::types::AudioCodec::Opus
                && format.codec_private.is_none()
            {
                CPacketType::OpusAudio
            } else {
                CPacketType::EncodedAudio
            }
        },
        streamkit_core::types::PacketType::RawVideo(_) => CPacketType::RawVideo,
        streamkit_core::types::PacketType::EncodedVideo(_) => CPacketType::EncodedVideo,
        streamkit_core::types::PacketType::Text => CPacketType::Text,
        streamkit_core::types::PacketType::Transcription => CPacketType::Transcription,
        streamkit_core::types::PacketType::Custom { .. } => CPacketType::Custom,
        streamkit_core::types::PacketType::Binary => CPacketType::Binary,
        streamkit_core::types::PacketType::Any | streamkit_core::types::PacketType::Passthrough => {
            CPacketType::Any
        },
    }
}

#[cfg(test)]
mod tests {
    use streamkit_core::types::{
        AudioCodec, AudioFormat, EncodedAudioFormat, PacketType, PixelFormat, RawVideoFormat,
        SampleFormat,
    };

    use super::*;

    #[test]
    fn output_format_pointers_reference_final_storage() {
        let meta = NodeMetadata::builder("multi_output")
            .output(
                "audio",
                PacketType::RawAudio(AudioFormat {
                    sample_rate: 48_000,
                    channels: 2,
                    sample_format: SampleFormat::F32,
                }),
            )
            .output(
                "video",
                PacketType::RawVideo(RawVideoFormat {
                    width: Some(1920),
                    height: Some(1080),
                    pixel_format: PixelFormat::Rgba8,
                }),
            )
            .build();

        let storage = PluginMetadataStorage::from_node_metadata(&meta);

        let audio_ptr = storage.outputs[0].produces_type.audio_format;
        let expected_audio_ptr =
            storage.output_audio_formats[0].as_ref().map_or(std::ptr::null(), std::ptr::from_ref);
        assert_eq!(audio_ptr, expected_audio_ptr);

        let video_ptr = storage.outputs[1].produces_type.raw_video_format;
        let expected_video_ptr =
            storage.output_video_formats[1].as_ref().map_or(std::ptr::null(), std::ptr::from_ref);
        assert_eq!(video_ptr, expected_video_ptr);

        assert!(matches!(
            conversions::packet_type_from_c(storage.outputs[0].produces_type),
            Ok(PacketType::RawAudio(_))
        ));
        assert!(matches!(
            conversions::packet_type_from_c(storage.outputs[1].produces_type),
            Ok(PacketType::RawVideo(_))
        ));
    }

    #[test]
    fn legacy_opus_metadata_does_not_set_custom_type_id() {
        let meta = NodeMetadata::builder("opus")
            .input(
                "in",
                &[PacketType::EncodedAudio(EncodedAudioFormat {
                    codec: AudioCodec::Opus,
                    codec_private: None,
                })],
            )
            .output(
                "out",
                PacketType::EncodedAudio(EncodedAudioFormat {
                    codec: AudioCodec::Opus,
                    codec_private: None,
                }),
            )
            .build();

        let storage = PluginMetadataStorage::from_node_metadata(&meta);

        assert_eq!(storage.inputs[0].accepts_types_count, 1);
        let input_type = unsafe { *storage.inputs[0].accepts_types };
        assert_eq!(input_type.type_discriminant, CPacketType::OpusAudio);
        assert!(input_type.custom_type_id.is_null());
        assert_eq!(storage.outputs[0].produces_type.type_discriminant, CPacketType::OpusAudio);
        assert!(storage.outputs[0].produces_type.custom_type_id.is_null());
    }
}
