// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

package main

import (
	"strings"
	"testing"
)

func TestLookupEncoder(t *testing.T) {
	if _, err := lookupEncoder(clipEncoders, "h264-sw"); err != nil {
		t.Fatalf("h264-sw should resolve: %v", err)
	}
	if _, err := lookupEncoder(castEncoders, "av1-hw"); err != nil {
		t.Fatalf("av1-hw should resolve: %v", err)
	}
	if _, err := lookupEncoder(clipEncoders, "nope"); err == nil {
		t.Fatal("unknown encoder should error")
	}
}

// Each profile must select the right encoder node and bitrate unit (software =
// kbps, hardware = bits/sec) and leave no unreplaced placeholders.
func TestClipEncoderProfiles(t *testing.T) {
	cases := map[string][]string{
		"h264-sw": {"video::openh264::encoder", "bitrate_kbps: 10000", "max_frame_rate: 30.0", "video_codec: h264"},
		"h264-hw": {"video::vulkan_video::h264_encoder", "bitrate: 10000000", "framerate: 30", "video_codec: h264"},
	}
	for name, wants := range cases {
		out := renderClipPipeline("https://example.com", 1920, 1080, 30, 300, 10000, clipEncoders[name])
		if strings.Contains(out, "{{") {
			t.Fatalf("%s: unreplaced placeholder in:\n%s", name, out)
		}
		for _, w := range wants {
			if !strings.Contains(out, w) {
				t.Errorf("%s: missing %q", name, w)
			}
		}
	}
}

func TestCastEncoderProfiles(t *testing.T) {
	cases := map[string][]string{
		"vp9-sw": {"video::vp9::encoder", "bitrate_kbps: 6000", `codecs="vp9"`},
		"av1-sw": {"video::svt_av1::encoder", "crf: 32", `codecs="av01.0.08M.08"`},
		"av1-hw": {"video::nv::av1_encoder", "bitrate: 6000000", `codecs="av01.0.08M.08"`},
	}
	for name, wants := range cases {
		out := renderCastPipeline("https://example.com", 1920, 1080, 30, 10, 6000, castEncoders[name])
		if strings.Contains(out, "{{") {
			t.Fatalf("%s: unreplaced placeholder in:\n%s", name, out)
		}
		for _, w := range wants {
			if !strings.Contains(out, w) {
				t.Errorf("%s: missing %q", name, w)
			}
		}
		if strings.Contains(out, "frame_count:") {
			t.Errorf("%s: cast must not bound frame_count", name)
		}
	}
}
