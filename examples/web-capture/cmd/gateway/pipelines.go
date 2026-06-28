// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

package main

import (
	_ "embed"
	"strconv"
	"strings"
)

// The clip (oneshot MP4) and cast (live WebM) pipelines are embedded as YAML
// templates from pipelines/. Each renders and encodes at one resolution (no
// downscale, so text stays crisp) and carries placeholders the render functions
// substitute: {{URL}} (strconv.Quote'd so a hostile URL can't break out of the
// scalar), dimensions/fps/bitrate, the {{ENCODER_BLOCK}} for the chosen profile,
// and the {{CONTENT_TYPE}}/{{MUX_CODEC}} it implies. See encoders.go.

//go:embed pipelines/clip.yml.tmpl
var clipPipelineTemplate string

//go:embed pipelines/cast.yml.tmpl
var castPipelineTemplate string

func renderClipPipeline(targetURL string, resW, resH, fps, frameCount, brKbps int, enc encoderProfile) string {
	tmpl := strings.Replace(clipPipelineTemplate, "{{ENCODER_BLOCK}}", enc.block, 1)
	return strings.NewReplacer(
		"{{URL}}", strconv.Quote(targetURL),
		"{{WIDTH}}", strconv.Itoa(resW),
		"{{HEIGHT}}", strconv.Itoa(resH),
		"{{FPS}}", strconv.Itoa(fps),
		"{{GOP}}", strconv.Itoa(gopForFPS(fps)),
		"{{FRAME_COUNT}}", strconv.Itoa(frameCount),
		"{{BR_KBPS}}", strconv.Itoa(brKbps),
		"{{BR_BPS}}", strconv.Itoa(brKbps*1000),
		"{{MUX_CODEC}}", enc.muxCodec,
		"{{CONTENT_TYPE}}", enc.contentType,
	).Replace(tmpl)
}

func renderCastPipeline(targetURL string, resW, resH, fps, maxClients, brKbps int, enc encoderProfile) string {
	tmpl := strings.Replace(castPipelineTemplate, "{{ENCODER_BLOCK}}", enc.block, 1)
	tmpl = strings.Replace(tmpl, "{{MUXER_BLOCK}}", enc.muxer, 1)
	return strings.NewReplacer(
		"{{URL}}", strconv.Quote(targetURL),
		"{{WIDTH}}", strconv.Itoa(resW),
		"{{HEIGHT}}", strconv.Itoa(resH),
		"{{FPS}}", strconv.Itoa(fps),
		"{{GOP}}", strconv.Itoa(gopForFPS(fps)),
		"{{MAX_CLIENTS}}", strconv.Itoa(maxClients),
		"{{BR_KBPS}}", strconv.Itoa(brKbps),
		"{{BR_BPS}}", strconv.Itoa(brKbps*1000),
		"{{CONTENT_TYPE}}", enc.contentType,
	).Replace(tmpl)
}

// gopForFPS picks a keyframe interval of ~1 second (one IDR per fps frames) so
// the fragmented-MP4 muxer flushes its first playable fragment quickly. Only
// the OpenH264 (software) block reads {{GOP}}; the Vulkan block forces its own
// keyframe at frame 0, so the substitution is a harmless no-op there.
func gopForFPS(fps int) int {
	if fps < 1 {
		return 1
	}
	return fps
}
