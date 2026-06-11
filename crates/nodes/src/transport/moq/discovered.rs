// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Per-pin codecs discovered from a remote publisher's MoQ catalog.
//!
//! Both `MoqPullNode` and `MoqPeerNode` create output pins dynamically when
//! downstream nodes connect to track-named pins.  The pin type must advertise
//! the codec the remote peer actually publishes — not the local config — so
//! catalog watchers record what they discover here and the pin-management
//! handlers consult the map when building pin definitions.

use std::collections::HashMap;
use std::sync::Arc;
use streamkit_core::types::{AudioCodec, VideoCodec};

/// A codec discovered for a specific track/pin from the remote catalog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DiscoveredCodec {
    Audio(AudioCodec),
    Video(VideoCodec),
}

/// Shared map of catalog-discovered codecs keyed by output pin name
/// (e.g. `video/hd`, `screen-input/video/hd`, `audio/data`).
///
/// Uses [`std::sync::RwLock`] rather than [`tokio::sync::RwLock`] because the
/// lock is never held across an `.await` point — only brief synchronous reads
/// and writes.
pub(super) type DiscoveredCodecs = Arc<std::sync::RwLock<HashMap<String, DiscoveredCodec>>>;

/// Record the codec the remote catalog advertises for an output pin.
///
/// Recovers from lock poisoning — see [`DiscoveredCodecs`] doc comment.
pub(super) fn record_discovered_codec(
    map: &DiscoveredCodecs,
    pin_name: &str,
    codec: DiscoveredCodec,
) {
    map.write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(pin_name.to_string(), codec);
}

/// Look up the catalog-advertised codec for an output pin, if any.
pub(super) fn discovered_codec(map: &DiscoveredCodecs, pin_name: &str) -> Option<DiscoveredCodec> {
    map.read().unwrap_or_else(std::sync::PoisonError::into_inner).get(pin_name).copied()
}

/// Forget the discovered codec for a removed output pin so a stale entry
/// can't leak into a pin recreated after a publisher reconnects with a
/// different codec.
pub(super) fn remove_discovered_codec(map: &DiscoveredCodecs, pin_name: &str) {
    map.write().unwrap_or_else(std::sync::PoisonError::into_inner).remove(pin_name);
}
