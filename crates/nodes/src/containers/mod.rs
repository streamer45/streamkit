// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use streamkit_core::NodeRegistry;

#[cfg(feature = "mp4")]
pub mod mp4;
pub mod ogg;
pub mod wav;
pub mod webm;

#[cfg(test)]
mod tests;

pub fn register_container_nodes(registry: &mut NodeRegistry) {
    #[cfg(feature = "mp4")]
    mp4::register_mp4_nodes(registry);
    ogg::register_ogg_nodes(registry);
    wav::register_wav_nodes(registry);
    webm::register_webm_nodes(registry);
}
