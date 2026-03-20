// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Shared pipeline YAML fixtures for compositor E2E tests.
 *
 * Extracted from compositor-context-menu.spec.ts, compositor-keyboard.spec.ts,
 * and compositor-perf.spec.ts to eliminate duplication.
 */

/**
 * Webcam PiP compositor pipeline YAML.
 *
 * Composites the user's webcam as picture-in-picture over colorbars with a
 * text overlay.  Used by all compositor E2E tests.
 */
export const WEBCAM_PIP_YAML = `
name: Webcam PiP (MoQ Stream)
description: Composites the user's webcam as picture-in-picture over colorbars with a text overlay
mode: dynamic

nodes:
  colorbars_bg:
    kind: video::colorbars
    params:
      width: 1280
      height: 720
      fps: 30
      draw_time: true

  moq_peer:
    kind: transport::moq::peer
    params:
      gateway_path: /moq/video
      input_broadcast: input
      output_broadcast: output
      allow_reconnect: true
    needs:
      in: opus_encoder
      in_1: vp9_encoder

  vp9_decoder:
    kind: video::vp9::decoder
    needs:
      in: moq_peer.out_1

  compositor:
    kind: video::compositor
    params:
      width: 1280
      height: 720
      num_inputs: 2
      layers:
        in_0:
          opacity: 1.0
          z_index: 0
        in_1:
          rect:
            x: 880
            y: 20
            width: 380
            height: 285
          opacity: 0.95
          z_index: 1
      text_overlays:
        - text: "Hello from StreamKit"
          rect:
            x: 40
            y: 660
            width: 400
            height: 40
          opacity: 1.0
          z_index: 2
          color: [255, 255, 255, 220]
          font_size: 28
          font_name: dejavu-sans-bold
    needs:
      - colorbars_bg
      - vp9_decoder

  pixel_convert:
    kind: video::pixel_convert
    params:
      output_format: nv12
    needs: compositor

  vp9_encoder:
    kind: video::vp9::encoder
    params:
      keyframe_interval: 30
    needs: pixel_convert

  opus_decoder:
    kind: audio::opus::decoder
    needs: moq_peer

  gain:
    kind: audio::gain
    params:
      gain: 1.0
    needs: opus_decoder

  opus_encoder:
    kind: audio::opus::encoder
    needs: gain
`.trim();

/**
 * Webcam PiP compositor pipeline with crop/zoom on the PiP layer.
 *
 * Same as {@link WEBCAM_PIP_YAML} but the `in_1` layer has crop_zoom=2.0
 * (2× zoom), crop_x=0.3, crop_y=0.7 to exercise the virtual PTZ controls.
 */
export const WEBCAM_PIP_CROPPED_YAML = `
name: Webcam PiP Cropped (MoQ Stream)
description: Composites the user's webcam as picture-in-picture over colorbars with crop/zoom
mode: dynamic

nodes:
  colorbars_bg:
    kind: video::colorbars
    params:
      width: 1280
      height: 720
      fps: 30
      draw_time: true

  moq_peer:
    kind: transport::moq::peer
    params:
      gateway_path: /moq/video
      input_broadcast: input
      output_broadcast: output
      allow_reconnect: true
    needs:
      in: opus_encoder
      in_1: vp9_encoder

  vp9_decoder:
    kind: video::vp9::decoder
    needs:
      in: moq_peer.out_1

  compositor:
    kind: video::compositor
    params:
      width: 1280
      height: 720
      num_inputs: 2
      layers:
        in_0:
          opacity: 1.0
          z_index: 0
        in_1:
          rect:
            x: 880
            y: 20
            width: 380
            height: 285
          opacity: 0.95
          z_index: 1
          crop_zoom: 2.0
          crop_x: 0.3
          crop_y: 0.7
      text_overlays:
        - text: "Hello from StreamKit"
          rect:
            x: 40
            y: 660
            width: 400
            height: 40
          opacity: 1.0
          z_index: 2
          color: [255, 255, 255, 220]
          font_size: 28
          font_name: dejavu-sans-bold
    needs:
      - colorbars_bg
      - vp9_decoder

  pixel_convert:
    kind: video::pixel_convert
    params:
      output_format: nv12
    needs: compositor

  vp9_encoder:
    kind: video::vp9::encoder
    params:
      keyframe_interval: 30
    needs: pixel_convert

  opus_decoder:
    kind: audio::opus::decoder
    needs: moq_peer

  gain:
    kind: audio::gain
    params:
      gain: 1.0
    needs: opus_decoder

  opus_encoder:
    kind: audio::opus::encoder
    needs: gain
`.trim();

/**
 * Webcam PiP compositor pipeline with circular crop on the PiP layer.
 *
 * Same as {@link WEBCAM_PIP_CROPPED_YAML} but the `in_1` layer uses
 * crop_shape=circle with a square rect for a perfect circle PiP overlay
 * (Loom-style).
 */
export const WEBCAM_PIP_CIRCLE_YAML = `
name: Webcam Circle PiP (MoQ Stream)
description: Composites the user's webcam as a circular picture-in-picture overlay over colorbars
mode: dynamic

nodes:
  colorbars_bg:
    kind: video::colorbars
    params:
      width: 1280
      height: 720
      fps: 30
      draw_time: true

  moq_peer:
    kind: transport::moq::peer
    params:
      gateway_path: /moq/video
      input_broadcast: input
      output_broadcast: output
      allow_reconnect: true
    needs:
      in: opus_encoder
      in_1: vp9_encoder

  vp9_decoder:
    kind: video::vp9::decoder
    needs:
      in: moq_peer.out_1

  compositor:
    kind: video::compositor
    params:
      width: 1280
      height: 720
      num_inputs: 2
      layers:
        in_0:
          opacity: 1.0
          z_index: 0
        in_1:
          rect:
            x: 1050
            y: 490
            width: 200
            height: 200
          opacity: 1.0
          z_index: 1
          crop_zoom: 1.8
          crop_x: 0.5
          crop_y: 0.4
          crop_shape: circle
      text_overlays:
        - text: "Hello from StreamKit"
          rect:
            x: 40
            y: 660
            width: 400
            height: 40
          opacity: 1.0
          z_index: 2
          color: [255, 255, 255, 220]
          font_size: 28
          font_name: dejavu-sans-bold
    needs:
      - colorbars_bg
      - vp9_decoder

  pixel_convert:
    kind: video::pixel_convert
    params:
      output_format: nv12
    needs: compositor

  vp9_encoder:
    kind: video::vp9::encoder
    params:
      keyframe_interval: 30
    needs: pixel_convert

  opus_decoder:
    kind: audio::opus::decoder
    needs: moq_peer

  gain:
    kind: audio::gain
    params:
      gain: 1.0
    needs: opus_decoder

  opus_encoder:
    kind: audio::opus::encoder
    needs: gain
`.trim();

/**
 * Two-colorbars compositor pipeline — no webcam or MoQ peer needed.
 *
 * Two colorbars sources composited together (PiP layout) and streamed
 * via a one-way MoQ push.  Useful for tests that need to verify the
 * compositor produces video output without requiring a WebTransport
 * publish connection from the browser.
 */
export const COMPOSITOR_COLORBARS_YAML = `
name: Video Compositor (MoQ Stream)
description: Composites two colorbars sources through the compositor node and streams via MoQ
mode: dynamic

nodes:
  colorbars_bg:
    kind: video::colorbars
    params:
      width: 1280
      height: 720
      fps: 30
      pixel_format: rgba8
      draw_time: true

  colorbars_pip:
    kind: video::colorbars
    params:
      width: 320
      height: 240
      fps: 30
      pixel_format: rgba8
      draw_time: true

  compositor:
    kind: video::compositor
    params:
      width: 1280
      height: 720
      num_inputs: 2
      layers:
        in_0:
          opacity: 1.0
          z_index: 0
        in_1:
          rect:
            x: 100
            y: 220
            width: 240
            height: 180
          opacity: 0.9
          z_index: 1
          rotation_degrees: 15.0
    needs:
      - colorbars_bg
      - colorbars_pip

  pixel_convert:
    kind: video::pixel_convert
    params:
      output_format: nv12
    needs: compositor

  vp9_encoder:
    kind: video::vp9::encoder
    needs: pixel_convert

  moq_peer:
    kind: transport::moq::peer
    params:
      gateway_path: /moq/video
      output_broadcast: output
      allow_reconnect: true
    needs: vp9_encoder
`.trim();
