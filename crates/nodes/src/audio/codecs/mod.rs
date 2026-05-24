// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use streamkit_core::NodeRegistry;

pub mod flac;
pub mod mp3;
pub mod opus;

pub fn register_audio_codecs(registry: &mut NodeRegistry) {
    opus::register_opus_nodes(registry);
    mp3::register_mp3_nodes(registry);
    flac::register_flac_nodes(registry);
}
