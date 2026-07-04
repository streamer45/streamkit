// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

package main

import (
	"fmt"
	"slices"
	"strings"
)

// encoderProfile is a swappable encoder choice. block is the `encoder` node.
// Clip pipelines use their fixed mp4 muxer (muxCodec = its video_codec); cast
// pipelines inject muxer (WebM for VP9/AV1, fMP4 for H.264). Software encoders
// take a kbps bitrate; the HW ones take bits/sec + framerate.
type encoderProfile struct {
	block       string
	muxer       string // cast only
	muxCodec    string // clip only
	contentType string
}

// Shared encoder node blocks; placeholders are filled at render time.
const (
	// gop_size = {{GOP}} (one keyframe per second) keeps the first fMP4
	// fragment short so playback starts within ~1s of first paint instead
	// of waiting a full default GOP (2s) for the muxer to flush.
	openh264Block = `  encoder:
    kind: video::openh264::encoder
    params:
      bitrate_kbps: {{BR_KBPS}}
      max_frame_rate: {{FPS}}.0
      gop_size: {{GOP}}
    needs: pixel_convert`

	vulkanH264Block = `  encoder:
    kind: video::vulkan_video::h264_encoder
    params:
      bitrate: {{BR_BPS}}
      framerate: {{FPS}}
    needs: pixel_convert`

	vp9Block = `  encoder:
    kind: video::vp9::encoder
    params:
      bitrate_kbps: {{BR_KBPS}}
    needs: pixel_convert`

	// CRF, not a target bitrate: SVT-AV1 rejects VBR in its low-delay mode
	// (required for live), so a bitrate_kbps>0 fails.
	svtAv1Block = `  encoder:
    kind: video::svt_av1::encoder
    params:
      bitrate_kbps: 0
      crf: 32
      fps: {{FPS}}
    needs: pixel_convert`

	nvAv1Block = `  encoder:
    kind: video::nv::av1_encoder
    params:
      bitrate: {{BR_BPS}}
      framerate: {{FPS}}
    needs: pixel_convert`
)

// Cast muxer blocks. WebM auto-detects VP9 vs AV1 from the packets; fMP4 (mp4
// mode:stream) carries H.264 and — since the http::mse fMP4 support landed —
// plays in Safari/iOS over a plain <video>.
const (
	webmMuxerBlock = `  muxer:
    kind: containers::webm::muxer
    params:
      video_width: {{WIDTH}}
      video_height: {{HEIGHT}}
      streaming_mode: live
    needs: encoder`

	fmp4MuxerBlock = `  muxer:
    kind: containers::mp4::muxer
    params:
      mode: stream
      video_width: {{WIDTH}}
      video_height: {{HEIGHT}}
      video_codec: h264
    needs: encoder`
)

// Plain MIME values; the pipeline renderers YAML-quote them at substitution
// time, and proxyMSE reuses them verbatim as a response-header fallback.
const (
	h264ContentType = `video/mp4; codecs="avc1.42c01f"`
	vp9ContentType  = `video/webm; codecs="vp9"`
	av1ContentType  = `video/webm; codecs="av01.0.08M.08"`
)

// clipEncoders stay H.264 so downloaded clips play everywhere; software is the
// default, hardware (Vulkan; NVIDIA/AMD/Intel) is opt-in.
var clipEncoders = map[string]encoderProfile{
	"h264-sw": {block: openh264Block, muxCodec: "h264", contentType: h264ContentType},
	"h264-hw": {block: vulkanH264Block, muxCodec: "h264", contentType: h264ContentType},
}

// castEncoders: VP9/AV1-in-WebM (crisper/efficient, Chromium + Firefox) or
// H.264-in-fMP4 (universal, including Safari/iOS).
var castEncoders = map[string]encoderProfile{
	"vp9-sw":  {block: vp9Block, muxer: webmMuxerBlock, contentType: vp9ContentType},
	"av1-sw":  {block: svtAv1Block, muxer: webmMuxerBlock, contentType: av1ContentType},
	"av1-hw":  {block: nvAv1Block, muxer: webmMuxerBlock, contentType: av1ContentType},
	"h264-sw": {block: openh264Block, muxer: fmp4MuxerBlock, contentType: h264ContentType},
	"h264-hw": {block: vulkanH264Block, muxer: fmp4MuxerBlock, contentType: h264ContentType},
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
