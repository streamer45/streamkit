# Testing the Video Compositor UI

## Overview
The video compositor node (`video::compositor`) has a visual canvas in the Design view where layers (input video, text overlays, image overlays) can be positioned, resized, and configured.

## Setup
1. Start backend: `SK_SERVER__MOQ_GATEWAY_URL=http://127.0.0.1:4545/moq SK_SERVER__ADDRESS=127.0.0.1:4545 just skit`
2. Start UI: `just ui`
3. Navigate to `http://localhost:3045/design`

## Loading a Compositor Pipeline
- The easiest way to get a compositor node on the canvas is to load a pre-built sample
- Click the **Samples** tab in the left sidebar
- Click **"Video Compositor (MoQ Stream)"** to load a pipeline with a compositor node and two colorbars inputs
- Another option: **"Webcam PiP (MoQ Stream)"** includes a compositor with a text overlay already configured
- Drag-and-drop from the Nodes library is difficult with browser automation; prefer loading samples

## Adding Text Overlays
- The compositor node has a **Layers** panel with an **"Add"** button
- The Add button may be offscreen in the node; use JavaScript `scrollIntoView()` + `click()` to interact with it
- Click **Add > Text** to add a text overlay
- The new text layer appears in the Layers list and on the canvas with a dashed bounding box

## Configuring Text Overlays
- Click a text layer in the Layers list to select it
- The inspector panel shows:
  - **Content**: textarea for the text string
  - **Size**: number input for font size (in pixels)
  - **Font**: dropdown with DejaVu font variants
  - **Color**: color picker
  - **Opacity**: slider
  - **Rotation**: preset buttons (0/90/180/270) and slider
  - **Mirror**: horizontal/vertical toggle buttons
- Use JavaScript `nativeInputValueSetter` + dispatching `input`/`change` events for React-controlled inputs

## Key Behavior to Verify
- Text should be aligned to the **top-left** of its bounding box (matching backend rendering from origin 0,0)
- Font size should be proportional to the bounding box (no double-scaling from CSS transform)
- The `CanvasInner` component applies `transform: scale(scale)` via CSS, so overlay content should use raw pixel values
- Bounding box should auto-expand height to fit text content

## Relevant Files
- `ui/src/components/CompositorCanvas.tsx` — Main canvas component with overlay rendering
- `ui/src/hooks/useCompositorLayers.ts` — Layer state management hook
- `crates/nodes/src/video/compositor/overlay.rs` — Backend text rendering (uses fontdue)
