// Copyright 2024 The ChromiumOS Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::codec::av1::parser::BitDepth;
use crate::codec::av1::parser::Profile;
use crate::encoder::PredictionStructure;
use crate::encoder::Tunings;
use crate::Resolution;

pub struct AV1;

#[derive(Clone)]
pub struct EncoderConfig {
    pub profile: Profile,
    pub bit_depth: BitDepth,
    pub resolution: Resolution,
    /// Display resolution (visible area) when it differs from coded resolution.
    /// Used to set `render_width`/`render_height` in the AV1 frame header so
    /// decoders crop superblock-alignment padding instead of showing black bars.
    pub display_resolution: Option<Resolution>,
    pub pred_structure: PredictionStructure,
    /// Initial tunings values
    pub initial_tunings: Tunings,
}

impl Default for EncoderConfig {
    fn default() -> Self {
        // Artificially encoder configuration with intent to be widely supported.
        Self {
            profile: Profile::Profile0,
            bit_depth: BitDepth::Depth8,
            resolution: Resolution { width: 320, height: 240 },
            display_resolution: None,
            pred_structure: PredictionStructure::LowDelay { limit: 1024 },
            initial_tunings: Default::default(),
        }
    }
}
