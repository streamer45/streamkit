// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

package main

import (
	"fmt"
	"slices"
	"strings"
)

// encoderProfile is a swappable encoder choice: the YAML for the `encoder` node
// (named so the muxer can `needs: encoder`), the mp4 `video_codec` it implies
// (ignored by the auto-detecting WebM muxer used for cast), and the content_type
// advertised to the client. Blocks carry {{FPS}}/{{BR_KBPS}}/{{BR_BPS}} — note
// the software encoders take a kbps bitrate while the HW encoders take bits/sec.
type encoderProfile struct {
	block       string
	muxCodec    string
	contentType string
}

// clipEncoders stay H.264 so downloaded clips remain universally playable
// (incl. Safari/iOS). Software is the default; hardware is Vulkan Video, which
// works on NVIDIA/AMD/Intel (NVENC exposes AV1 only, not H.264).
var clipEncoders = map[string]encoderProfile{
	"h264-sw": {
		block: `  encoder:
    kind: video::openh264::encoder
    params:
      bitrate_kbps: {{BR_KBPS}}
      max_frame_rate: {{FPS}}.0
    needs: pixel_convert`,
		muxCodec:    "h264",
		contentType: `'video/mp4; codecs="avc1.42c01f"'`,
	},
	"h264-hw": {
		block: `  encoder:
    kind: video::vulkan_video::h264_encoder
    params:
      bitrate: {{BR_BPS}}
      framerate: {{FPS}}
    needs: pixel_convert`,
		muxCodec:    "h264",
		contentType: `'video/mp4; codecs="avc1.42c01f"'`,
	},
}

// castEncoders: VP9 (software, default) or AV1 — software (svt, for local
// testing without a GPU) or hardware (NVENC). The WebM muxer auto-detects the
// codec from the packets, so only the encoder and advertised codec string vary.
var castEncoders = map[string]encoderProfile{
	"vp9-sw": {
		block: `  encoder:
    kind: video::vp9::encoder
    params:
      bitrate_kbps: {{BR_KBPS}}
    needs: pixel_convert`,
		contentType: `'video/webm; codecs="vp9"'`,
	},
	"av1-sw": {
		// CRF, not a target bitrate: SVT-AV1 rejects VBR in its low-delay mode
		// (required for live), so a bitrate_kbps>0 fails. CRF works in low-delay.
		block: `  encoder:
    kind: video::svt_av1::encoder
    params:
      bitrate_kbps: 0
      crf: 32
      fps: {{FPS}}
    needs: pixel_convert`,
		contentType: `'video/webm; codecs="av01.0.08M.08"'`,
	},
	"av1-hw": {
		block: `  encoder:
    kind: video::nv::av1_encoder
    params:
      bitrate: {{BR_BPS}}
      framerate: {{FPS}}
    needs: pixel_convert`,
		contentType: `'video/webm; codecs="av01.0.08M.08"'`,
	},
}

func lookupEncoder(profiles map[string]encoderProfile, name string) (encoderProfile, error) {
	if p, ok := profiles[name]; ok {
		return p, nil
	}
	keys := make([]string, 0, len(profiles))
	for k := range profiles {
		keys = append(keys, k)
	}
	slices.Sort(keys)
	return encoderProfile{}, fmt.Errorf("unknown encoder %q (choices: %s)", name, strings.Join(keys, ", "))
}
