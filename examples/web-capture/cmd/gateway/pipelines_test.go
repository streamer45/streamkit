// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

package main

import (
	"strconv"
	"strings"
	"testing"
)

func TestRenderClipPipeline(t *testing.T) {
	out := renderClipPipeline("https://example.com/p?a=1", 1920, 1080, 30, 300, 10000, clipEncoders["h264-sw"])
	if strings.Contains(out, "{{") {
		t.Fatalf("unreplaced placeholder in:\n%s", out)
	}
	for _, want := range []string{
		`url: "https://example.com/p?a=1"`,
		"width: 1920",
		"height: 1080",
		"frame_count: 300",
		"mode: stream",
		"video_codec: h264",
		"bitrate_kbps: 10000",
		"max_frame_rate: 30.0",
		"plugin::native::servo",
		"output_format: nv12",
		"streamkit::http_output",
	} {
		if !strings.Contains(out, want) {
			t.Errorf("clip pipeline missing %q", want)
		}
	}
}

// A hostile URL must not break out of the YAML scalar — strconv.Quote escapes
// any embedded newline/quote so injected keys never appear at line start.
func TestRenderClipPipelineQuotesURL(t *testing.T) {
	malicious := "https://x/\"\n  evil: true"
	out := renderClipPipeline(malicious, 1920, 1080, 30, 30, 10000, clipEncoders["h264-sw"])
	if strings.Contains(out, "\n  evil: true") {
		t.Errorf("newline injection not escaped:\n%s", out)
	}
	if !strings.Contains(out, "url: "+strconv.Quote(malicious)) {
		t.Errorf("url not quoted as expected:\n%s", out)
	}
}

func TestRenderCastPipeline(t *testing.T) {
	out := renderCastPipeline("https://example.com", 1920, 1080, 30, 10, 6000, castEncoders["vp9-sw"])
	if strings.Contains(out, "{{") {
		t.Fatalf("unreplaced placeholder in:\n%s", out)
	}
	for _, want := range []string{
		"mse_path: /video",
		"transport::http::mse",
		"video::vp9::encoder",
		"max_clients: 10",
		"streaming_mode: live",
		"width: 1920",
		"height: 1080",
	} {
		if !strings.Contains(out, want) {
			t.Errorf("cast pipeline missing %q", want)
		}
	}
	if strings.Contains(out, "frame_count:") {
		t.Error("cast pipeline must not bound frame_count (live is infinite)")
	}
}
