// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

package main

import (
	"strconv"
	"strings"
)

// Both pipelines render a web page with the Servo plugin. Servo emits RGBA8, so
// pixel_convert -> nv12 is mandatory before any encoder. The target URL is
// strconv.Quote'd into the YAML so a hostile URL cannot break out of the scalar.
//
// The page renders and is encoded at the SAME resolution (no viewport_* override
// => Servo's viewport defaults to the output size), so there is no downscale to
// blur text. A wider resolution also gives the page a roomier desktop layout.
//
// The encoder is a swappable profile (software default, hardware opt-in): the
// {{ENCODER_BLOCK}} placeholder is replaced with the chosen encoder node, and
// {{CONTENT_TYPE}} / {{MUX_CODEC}} follow from it. See encoders.go.

// clip: bounded oneshot -> fragmented MP4 (mode: stream = moov-up-front, so it
// plays inline as it renders) returned over http_output.
const clipPipelineTemplate = `
name: web-clip
description: Render a web page to a short MP4 clip
mode: oneshot
attributes:
  service: web-capture
  mode: clip
client:
  input:
    type: none
  output:
    type: video
nodes:
  web:
    kind: plugin::native::servo
    params:
      url: {{URL}}
      width: {{WIDTH}}
      height: {{HEIGHT}}
      fps: {{FPS}}
      frame_count: {{FRAME_COUNT}}
  pixel_convert:
    kind: video::pixel_convert
    params:
      output_format: nv12
    needs: web
{{ENCODER_BLOCK}}
  muxer:
    kind: containers::mp4::muxer
    params:
      mode: stream
      video_width: {{WIDTH}}
      video_height: {{HEIGHT}}
      video_codec: {{MUX_CODEC}}
    needs: encoder
  http_output:
    kind: streamkit::http_output
    params:
      content_type: {{CONTENT_TYPE}}
    needs: muxer
`

// cast: unbounded dynamic session -> live WebM over HTTP MSE. frame_count is
// omitted (0 = infinite). The WebM muxer auto-detects the codec (VP9/AV1) from
// the encoded packets, so only the encoder and the advertised content_type
// change between profiles.
const castPipelineTemplate = `
name: web-cast
description: Render a web page to a live WebM stream
mode: dynamic
attributes:
  service: web-capture
  mode: cast
client:
  watch:
    mse_path: /video
    video: true
nodes:
  web:
    kind: plugin::native::servo
    params:
      url: {{URL}}
      width: {{WIDTH}}
      height: {{HEIGHT}}
      fps: {{FPS}}
  pixel_convert:
    kind: video::pixel_convert
    params:
      output_format: nv12
    needs: web
{{ENCODER_BLOCK}}
  muxer:
    kind: containers::webm::muxer
    params:
      video_width: {{WIDTH}}
      video_height: {{HEIGHT}}
      streaming_mode: live
    needs: encoder
  http_mse:
    kind: transport::http::mse
    params:
      path: /video
      max_clients: {{MAX_CLIENTS}}
      content_type: {{CONTENT_TYPE}}
    needs: muxer
`

func renderClipPipeline(targetURL string, resW, resH, fps, frameCount, brKbps int, enc encoderProfile) string {
	tmpl := strings.Replace(clipPipelineTemplate, "{{ENCODER_BLOCK}}", enc.block, 1)
	return strings.NewReplacer(
		"{{URL}}", strconv.Quote(targetURL),
		"{{WIDTH}}", strconv.Itoa(resW),
		"{{HEIGHT}}", strconv.Itoa(resH),
		"{{FPS}}", strconv.Itoa(fps),
		"{{FRAME_COUNT}}", strconv.Itoa(frameCount),
		"{{BR_KBPS}}", strconv.Itoa(brKbps),
		"{{BR_BPS}}", strconv.Itoa(brKbps*1000),
		"{{MUX_CODEC}}", enc.muxCodec,
		"{{CONTENT_TYPE}}", enc.contentType,
	).Replace(tmpl)
}

func renderCastPipeline(targetURL string, resW, resH, fps, maxClients, brKbps int, enc encoderProfile) string {
	tmpl := strings.Replace(castPipelineTemplate, "{{ENCODER_BLOCK}}", enc.block, 1)
	return strings.NewReplacer(
		"{{URL}}", strconv.Quote(targetURL),
		"{{WIDTH}}", strconv.Itoa(resW),
		"{{HEIGHT}}", strconv.Itoa(resH),
		"{{FPS}}", strconv.Itoa(fps),
		"{{MAX_CLIENTS}}", strconv.Itoa(maxClients),
		"{{BR_KBPS}}", strconv.Itoa(brKbps),
		"{{BR_BPS}}", strconv.Itoa(brKbps*1000),
		"{{CONTENT_TYPE}}", enc.contentType,
	).Replace(tmpl)
}
