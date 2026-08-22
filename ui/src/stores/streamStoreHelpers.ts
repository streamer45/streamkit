// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/** Helpers for streamStore — connection lifecycle, teardown, and health warnings. */

import * as Moq from '@moq/net';
import * as Publish from '@moq/publish';
import type { Getter } from '@moq/signals';
import { Effect, Signal } from '@moq/signals';
import * as Watch from '@moq/watch';

import { AV1_CODEC_STRING } from '@/constants/codecs';
import type { PublishTrackConfig } from '@/types/types';

import type {
  CameraStatus,
  ConnectionStatus,
  MicStatus,
  VideoSourceType,
  WatchStatus,
} from './streamStore';
import { getLogger } from '../utils/logger';

const logger = getLogger('streamStore');

/** Recognized codec values for validation. */
const SUPPORTED_VIDEO_CODECS = ['vp9', 'av1'];
const SUPPORTED_AUDIO_CODECS = ['opus'];

/** Map user-facing codec name to WebCodecs codec string. */
function mapCodecToWebCodecs(codec: string): string {
  switch (codec) {
    case 'vp9':
      return 'vp09';
    case 'av1':
      return AV1_CODEC_STRING;
    default:
      throw new Error(
        `Unsupported video codec '${codec}'. Supported codecs: ${SUPPORTED_VIDEO_CODECS.join(', ')}.`
      );
  }
}

/** Build `@moq/publish` encoder config and capture constraints from per-track media hints. */
export function buildVideoEncoderConfig(track?: PublishTrackConfig | null): {
  encoderConfig: { codec: string; maxPixels?: number; maxBitrate?: number };
  constraints?: { width?: number; height?: number };
} {
  const codec = mapCodecToWebCodecs(track?.codec ?? 'vp9');
  const encoderConfig: { codec: string; maxPixels?: number; maxBitrate?: number } = { codec };

  const width = track?.width ?? undefined;
  const height = track?.height ?? undefined;

  if (width != null && height != null) {
    if (width === 0 || height === 0) {
      logger.warn(
        `Track (source=${track?.source}) has zero dimension (${width}x${height}) — skipping maxPixels`
      );
    } else {
      encoderConfig.maxPixels = width * height;
    }
  } else if (width != null || height != null) {
    logger.warn(
      `Track (source=${track?.source}) has partial dimensions ` +
        `(width=${width ?? 'unset'}, height=${height ?? 'unset'}) — ` +
        `maxPixels will not be computed; set both for correct resolution control`
    );
  }
  if (track?.max_bitrate != null) {
    if (track.max_bitrate === 0) {
      logger.warn(`Track (source=${track.source}) has max_bitrate: 0 — skipping maxBitrate`);
    } else {
      // Convert kilobits per second → bits per second for the encoder.
      encoderConfig.maxBitrate = track.max_bitrate * 1000;
    }
  }

  const constraints: { width?: number; height?: number } = {};
  if (width != null && width > 0) constraints.width = width;
  if (height != null && height > 0) constraints.height = height;

  return {
    encoderConfig,
    constraints: Object.keys(constraints).length > 0 ? constraints : undefined,
  };
}

export function validateTrackCodecs(tracks: PublishTrackConfig[]): void {
  for (const track of tracks) {
    if (track.codec == null) continue;
    const supported = track.kind === 'video' ? SUPPORTED_VIDEO_CODECS : SUPPORTED_AUDIO_CODECS;
    if (!supported.includes(track.codec)) {
      logger.warn(
        `Track (kind=${track.kind}, source=${track.source}) has unrecognized ` +
          `codec '${track.codec}'; supported: ${supported.join(', ')}`
      );
    }
  }
}

export type ConnectDecision =
  | {
      ok: true;
      trimmedServerUrl: string;
      shouldWatch: boolean;
      shouldPublish: boolean;
    }
  | { ok: false; errorMessage: string };

/** Track names must match the pins expected by the `transport::moq::peer`
 *  node and the sample pipelines (e.g. `moq_peer.video/hd`). */
export const VIDEO_TRACK_NAME = 'video/hd';
export const AUDIO_TRACK_NAME = 'audio/data';

/**
 * Owns the renderer plus the writable canvas signal, preserving the
 * `videoRenderer.canvas.set(el)` binding contract used by the views.
 */
export class VideoRendererHandle {
  readonly canvas = new Signal<HTMLCanvasElement | undefined>(undefined);
  readonly renderer: Watch.Video.Renderer;

  constructor(decoder: Watch.Video.Decoder) {
    this.renderer = new Watch.Video.Renderer(decoder, { canvas: this.canvas });
  }

  close(): void {
    this.renderer.close();
  }
}

/**
 * Owns the emitter plus writable muted/volume signals, preserving the
 * `audioEmitter.muted` / `audioEmitter.volume` contract used by the views.
 */
export class AudioEmitterHandle {
  readonly muted = new Signal(false);
  readonly volume = new Signal(0.5);
  readonly emitter: Watch.Audio.Emitter;

  constructor(decoder: Watch.Audio.Decoder) {
    this.emitter = new Watch.Audio.Emitter(decoder, {
      muted: this.muted,
      volume: this.volume,
    });
  }

  close(): void {
    this.emitter.close();
  }
}

/** Owns a capture source plus the writable enabled signal used by the toggles. */
export class MicrophoneHandle {
  readonly enabled = new Signal(true);
  readonly inner: Publish.Source.Microphone;

  constructor() {
    this.inner = new Publish.Source.Microphone({ enabled: this.enabled });
  }

  get source(): Getter<Publish.Audio.Source | undefined> {
    return this.inner.out.source;
  }

  close(): void {
    this.inner.close();
  }
}

export class CameraHandle {
  readonly enabled = new Signal(true);
  readonly inner: Publish.Source.Camera;

  constructor(constraints?: { width?: number; height?: number }) {
    this.inner = new Publish.Source.Camera({ enabled: this.enabled, constraints });
  }

  get source(): Getter<Publish.Video.Source | undefined> {
    return this.inner.out.source;
  }

  close(): void {
    this.inner.close();
  }
}

export class ScreenHandle {
  readonly enabled = new Signal(true);
  readonly inner: Publish.Source.Screen;

  constructor(videoConstraints?: { width?: number; height?: number }) {
    this.inner = new Publish.Source.Screen({ enabled: this.enabled, video: videoConstraints });
  }

  get source(): Getter<{ audio?: Publish.Audio.Source; video?: Publish.Video.Source } | undefined> {
    return this.inner.out.source;
  }

  close(): void {
    this.inner.close();
  }
}

/**
 * A published broadcast plus its encoders. `@moq/publish` splits the old
 * monolithic Broadcast config into per-rendition encoders wired to a shared
 * capture, so this handle owns the whole publish graph for teardown and
 * exposes the writable enable signals the store toggles need.
 */
export class PublishHandle {
  readonly broadcast: Publish.Broadcast;
  readonly capture: Publish.Video.Capture | null;
  readonly video: Publish.Video.Encoder | null;
  readonly audio: { enabled: Signal<boolean>; encoder: Publish.Audio.Encoder } | null;

  constructor(opts: {
    broadcast: Publish.Broadcast;
    capture?: Publish.Video.Capture | null;
    video?: Publish.Video.Encoder | null;
    audio?: { enabled: Signal<boolean>; encoder: Publish.Audio.Encoder } | null;
  }) {
    this.broadcast = opts.broadcast;
    this.capture = opts.capture ?? null;
    this.video = opts.video ?? null;
    this.audio = opts.audio ?? null;
  }

  close(): void {
    this.video?.close();
    this.audio?.encoder.close();
    this.capture?.close();
    this.broadcast.close();
  }
}

export type ConnectAttempt = {
  connection: Moq.Connection.Reload | null;
  healthEffect: Effect | null;
  watch: Watch.Broadcast | null;
  watchSync: Watch.Sync | null;
  audioSource: Watch.Audio.Source | null;
  audioDecoder: Watch.Audio.Decoder | null;
  audioEmitter: AudioEmitterHandle | null;
  videoSource: Watch.Video.Source | null;
  videoDecoder: Watch.Video.Decoder | null;
  videoRenderer: VideoRendererHandle | null;
  microphone: MicrophoneHandle | null;
  camera: CameraHandle | null;
  screen: ScreenHandle | null;
  publish: PublishHandle | null;
  /** Secondary publish broadcast for multi-broadcast mode (e.g. camera PiP). */
  secondaryPublish: PublishHandle | null;
  /** Secondary camera source for multi-broadcast mode. */
  secondaryCamera: CameraHandle | null;
  /** Secondary screen source for multi-broadcast mode. */
  secondaryScreen: ScreenHandle | null;
};

/** Minimal slice of StreamState needed by helper functions. */
export interface ConnectableState {
  connectionMode: 'session' | 'direct';
  enablePublish: boolean;
  enableWatch: boolean;
  serverUrl: string;
  moqToken: string;
  inputBroadcast: string;
  outputBroadcast: string;
  pipelineNeedsAudio: boolean;
  pipelineNeedsVideo: boolean;
  pipelineOutputsAudio: boolean;
  pipelineOutputsVideo: boolean;
  /** True when the pipeline uses separate publisher/subscriber nodes via an
   *  external MoQ relay, as opposed to a gateway `transport::moq::peer` node
   *  managed directly by skit. */
  isExternalRelay: boolean;
  /** The video capture source: 'camera' (getUserMedia) or 'screen' (getDisplayMedia). */
  videoSourceType: VideoSourceType;
  /** Parsed publish tracks from the client section (for multi-broadcast). */
  tracks: PublishTrackConfig[];
  /** All unique broadcast names derived from tracks. */
  publishBroadcasts: string[];
  status: ConnectionStatus;
  errorMessage: string;
  isMicEnabled: boolean;
  isCameraEnabled: boolean;
  micStatus: MicStatus;
  cameraStatus: CameraStatus;
  watchStatus: WatchStatus;
  isSecondaryCameraEnabled: boolean;
  secondaryCameraStatus: CameraStatus;
  /** Human-readable label for the current phase of a connect attempt
   *  (e.g. 'devices', 'relay', 'pipeline').  Empty when idle. */
  connectingStep: string;
}

type StateSetter = (partial: Partial<ConnectableState>) => void;

/** All MoQ resource references reset to null — used when disconnecting or on error. */
export const NULL_MOQ_REFS = {
  publish: null,
  watch: null,
  watchSync: null,
  audioSource: null,
  audioDecoder: null,
  audioEmitter: null,
  videoSource: null,
  videoDecoder: null,
  videoRenderer: null,
  connection: null,
  microphone: null,
  camera: null,
  screen: null,
  healthEffect: null,
  secondaryPublish: null,
  secondaryCamera: null,
  secondaryScreen: null,
} as const;

export function waitForSignalValue<T>(
  signal: Getter<T>,
  predicate: (value: T) => boolean,
  timeoutMs: number,
  timeoutMessage: string,
  abortSignal?: AbortSignal
): Promise<T> {
  if (abortSignal?.aborted) {
    return Promise.reject(new DOMException('Aborted', 'AbortError'));
  }

  const initial = signal.peek();
  if (predicate(initial)) {
    return Promise.resolve(initial);
  }

  return new Promise((resolve, reject) => {
    let dispose: () => void = () => {};

    const cleanup = () => {
      clearTimeout(timeoutId);
      dispose();
    };

    const timeoutId = setTimeout(() => {
      cleanup();
      reject(new Error(timeoutMessage));
    }, timeoutMs);

    if (abortSignal) {
      abortSignal.addEventListener(
        'abort',
        () => {
          cleanup();
          reject(new DOMException('Aborted', 'AbortError'));
        },
        { once: true }
      );
    }

    dispose = signal.subscribe((value) => {
      if (predicate(value)) {
        cleanup();
        resolve(value);
      }
    });
  });
}

/** Wait for a broadcast to appear on the relay before subscribing. */
async function waitForBroadcastAnnouncement(
  connection: Moq.Connection.Reload,
  broadcastName: string,
  timeoutMs = 15_000,
  abortSignal?: AbortSignal
): Promise<void> {
  if (abortSignal?.aborted) throw new DOMException('Aborted', 'AbortError');
  if (!connection.established.peek()) return;
  logger.info(`Waiting for broadcast '${broadcastName}' announcement...`);
  const announcements = connection.announced();
  const deadline = Date.now() + timeoutMs;
  try {
    const abortPromise = abortSignal
      ? new Promise<never>((_, reject) => {
          abortSignal.addEventListener(
            'abort',
            () => reject(new DOMException('Aborted', 'AbortError')),
            { once: true }
          );
        })
      : null;

    while (Date.now() < deadline) {
      const remaining = deadline - Date.now();
      const racers: Promise<unknown>[] = [
        announcements.next(),
        new Promise<null>((r) => setTimeout(() => r(null), remaining)),
      ];
      if (abortPromise) racers.push(abortPromise);

      const entry = (await Promise.race(racers)) as Awaited<
        ReturnType<typeof announcements.next>
      > | null;
      if (!entry) break;
      if (entry.active && entry.path.toString() === broadcastName) {
        logger.info(`Broadcast '${broadcastName}' announced`);
        return;
      }
    }
    logger.warn(`Broadcast '${broadcastName}' not announced within ${timeoutMs}ms, proceeding`);
  } finally {
    announcements.close();
  }
}

export function decideConnect(
  state: Pick<ConnectableState, 'connectionMode' | 'enablePublish' | 'enableWatch' | 'serverUrl'>
): ConnectDecision {
  const trimmedServerUrl = state.serverUrl.trim();
  if (!trimmedServerUrl) {
    return {
      ok: false,
      errorMessage: 'Missing MoQ Gateway URL. Configure it on the server or enter one above.',
    };
  }

  if (state.connectionMode === 'direct' && !state.enablePublish && !state.enableWatch) {
    return { ok: false, errorMessage: 'At least one of Publish or Watch must be enabled.' };
  }

  const shouldWatch = state.connectionMode === 'session' || state.enableWatch;
  // Session mode no longer implicitly enables publishing — driven entirely
  // by `enablePublish` which session setup sets based on pipeline inputs.
  const shouldPublish = state.enablePublish;

  return { ok: true, trimmedServerUrl, shouldWatch, shouldPublish };
}

export function formatConnectError(error: unknown): string {
  return error instanceof Error
    ? `Connection failed: ${error.message}`
    : 'Failed to connect to MoQ server. Please check your connection and try again.';
}

/** Shut down a media source that may expose `.close()` or only `.enabled`. */
function shutdownMediaSource(source: MicrophoneHandle | CameraHandle | ScreenHandle | null): void {
  if (!source) return;
  if (typeof source.close === 'function') {
    source.close();
  } else if (source.enabled) {
    source.enabled.set(false);
  }
}

/** Ordered list of ConnectAttempt keys whose values expose `.close()`. */
const CLOSEABLE_KEYS: ReadonlyArray<keyof ConnectAttempt> = [
  'healthEffect',
  'secondaryPublish',
  'publish',
  'videoRenderer',
  'videoDecoder',
  'videoSource',
  'audioEmitter',
  'audioDecoder',
  'audioSource',
  'watchSync',
  'watch',
  'connection',
] as const;

export function cleanupConnectAttempt(attempt: ConnectAttempt): void {
  for (const key of CLOSEABLE_KEYS) {
    const resource = attempt[key];
    if (resource && typeof (resource as { close?: () => void }).close === 'function') {
      (resource as { close: () => void }).close();
    }
  }
  shutdownMediaSource(attempt.microphone);
  shutdownMediaSource(attempt.camera);
  shutdownMediaSource(attempt.screen);
  shutdownMediaSource(attempt.secondaryCamera);
  shutdownMediaSource(attempt.secondaryScreen);
}

function setupConnectionStatusSync(
  healthEffect: Effect,
  connection: Moq.Connection.Reload,
  get: () => ConnectableState,
  set: StateSetter
): void {
  let hadConnected = false;
  healthEffect.subscribe(connection.status, (value) => {
    const current = get().status;
    const mapped: ConnectionStatus =
      value === 'connected' ? 'connected' : value === 'connecting' ? 'connecting' : 'disconnected';

    if (value === 'connected') {
      hadConnected = true;
    }

    if (!hadConnected && current === 'connecting' && mapped === 'disconnected') {
      return;
    }

    set({ status: mapped });
    if (mapped === 'disconnected' && current === 'connected') {
      set({
        errorMessage:
          'Disconnected from MoQ gateway. Check the URL, relay availability, and your network.',
      });
    }
  });
}

function setupWatchPath(
  healthEffect: Effect,
  connection: Moq.Connection.Reload,
  outputBroadcast: string,
  outputsAudio: boolean,
  outputsVideo: boolean,
  set: StateSetter
): {
  watch: Watch.Broadcast;
  watchSync: Watch.Sync;
  audioSource: Watch.Audio.Source | null;
  audioDecoder: Watch.Audio.Decoder | null;
  audioEmitter: AudioEmitterHandle | null;
  videoSource: Watch.Video.Source | null;
  videoDecoder: Watch.Video.Decoder | null;
  videoRenderer: VideoRendererHandle | null;
} {
  logger.info('Step 2: Creating watch broadcast (subscribe FIRST)');
  const watch = new Watch.Broadcast({
    connection: connection.established,
    enabled: true,
    name: Moq.Path.from(outputBroadcast),
  });

  let audioSource: Watch.Audio.Source | null = null;
  let videoSource: Watch.Video.Source | null = null;

  if (outputsAudio) {
    logger.info('Step 3: Creating audio source');
    audioSource = new Watch.Audio.Source({
      broadcast: watch,
      supported: Watch.Audio.Decoder.supported,
    });
  }
  if (outputsVideo) {
    logger.info('Step 3b: Creating video source');
    videoSource = new Watch.Video.Source({
      broadcast: watch,
      supported: Watch.Video.Decoder.supported,
    });
  }

  const watchSync = new Watch.Sync({
    connection: connection.established,
    audio: audioSource?.out.jitter,
    video: videoSource?.out.jitter,
  });

  let audioDecoder: Watch.Audio.Decoder | null = null;
  let audioEmitter: AudioEmitterHandle | null = null;
  if (audioSource) {
    audioDecoder = new Watch.Audio.Decoder(audioSource, watchSync, { enabled: true });
    audioEmitter = new AudioEmitterHandle(audioDecoder);
  }

  let videoDecoder: Watch.Video.Decoder | null = null;
  let videoRenderer: VideoRendererHandle | null = null;
  if (videoSource) {
    videoDecoder = new Watch.Video.Decoder(videoSource, watchSync, { enabled: true });
    videoRenderer = new VideoRendererHandle(videoDecoder);
  }

  set({ watchStatus: watch.out.status.peek() });
  healthEffect.subscribe(watch.out.status, (value) => {
    set({ watchStatus: value });
  });

  return {
    watch,
    watchSync,
    audioSource,
    audioDecoder,
    audioEmitter,
    videoSource,
    videoDecoder,
    videoRenderer,
  };
}

/** Set up screen capture with 30s timeout for the OS picker dialog.
 *  NOTE: Screen uses `screenProps.video` for constraints while Camera uses
 *  `cameraProps.constraints` — this asymmetry is correct per the @moq/publish
 *  types (Publish.Source.Screen vs Publish.Source.Camera have different APIs). */
async function setupScreenCapture(
  healthEffect: Effect,
  microphone: MicrophoneHandle | null,
  set: StateSetter,
  abortSignal?: AbortSignal,
  videoConstraints?: { width?: number; height?: number }
): Promise<ScreenHandle> {
  const screen = new ScreenHandle(videoConstraints);
  set({ cameraStatus: screen.source.peek()?.video ? 'ready' : 'requesting' });
  healthEffect.subscribe(screen.source, (v) =>
    set({ cameraStatus: v?.video ? 'ready' : 'requesting' })
  );
  if (!screen.source.peek()?.video) {
    try {
      await waitForSignalValue(
        screen.source,
        (v) => v?.video !== undefined,
        30_000,
        'Screen capture not available',
        abortSignal
      );
    } catch (e) {
      shutdownMediaSource(screen);
      shutdownMediaSource(microphone);
      throw e;
    }
  }
  return screen;
}

/** Set up camera capture with 15s timeout for permission dialog. */
async function setupCameraCapture(
  healthEffect: Effect,
  microphone: MicrophoneHandle | null,
  set: StateSetter,
  abortSignal?: AbortSignal,
  videoConstraints?: { width?: number; height?: number }
): Promise<CameraHandle> {
  const camera = new CameraHandle(videoConstraints);
  set({ cameraStatus: camera.source.peek() ? 'ready' : 'requesting' });
  healthEffect.subscribe(camera.source, (v) => set({ cameraStatus: v ? 'ready' : 'requesting' }));
  if (!camera.source.peek()) {
    try {
      await waitForSignalValue(
        camera.source,
        (v) => v !== undefined,
        15_000,
        'Camera not available',
        abortSignal
      );
    } catch (e) {
      shutdownMediaSource(camera);
      shutdownMediaSource(microphone);
      throw e;
    }
  }
  return camera;
}

async function setupMediaSources(
  healthEffect: Effect,
  needsAudio: boolean,
  needsVideo: boolean,
  videoSourceType: VideoSourceType,
  set: StateSetter,
  abortSignal?: AbortSignal,
  videoConstraints?: { width?: number; height?: number }
): Promise<{
  microphone: MicrophoneHandle | null;
  camera: CameraHandle | null;
  screen: ScreenHandle | null;
}> {
  let microphone: MicrophoneHandle | null = null;
  let camera: CameraHandle | null = null;
  let screen: ScreenHandle | null = null;
  if (needsAudio) {
    microphone = new MicrophoneHandle();
    set({ micStatus: microphone.source.peek() ? 'ready' : 'requesting' });
    healthEffect.subscribe(microphone.source, (v) =>
      set({ micStatus: v ? 'ready' : 'requesting' })
    );
  }
  if (needsVideo) {
    if (videoSourceType === 'screen') {
      screen = await setupScreenCapture(
        healthEffect,
        microphone,
        set,
        abortSignal,
        videoConstraints
      );
    } else {
      camera = await setupCameraCapture(
        healthEffect,
        microphone,
        set,
        abortSignal,
        videoConstraints
      );
    }
  }
  return { microphone, camera, screen };
}

/** Build the shared video capture from a screen or camera handle.
 *  Screen capture: derive a video-only signal from the composite
 *  Screen.source signal ({ audio?, video? } | undefined). System audio from
 *  screen capture is ignored — mic remains the sole audio source. */
function createVideoCapture(
  healthEffect: Effect,
  screen: ScreenHandle | null,
  camera: CameraHandle | null
): Publish.Video.Capture | null {
  if (screen) {
    const videoOnlySignal = new Signal<Publish.Video.Source | undefined>(
      screen.source.peek()?.video
    );
    healthEffect.subscribe(screen.source, (v) => videoOnlySignal.set(v?.video));
    return new Publish.Video.Capture({ source: videoOnlySignal });
  }
  if (camera) {
    return new Publish.Video.Capture({ source: camera.source });
  }
  return null;
}

/** Construct the publish encoders, closing the already-created capture and
 *  broadcast if either encoder constructor throws so nothing leaks before a
 *  PublishHandle owns them. */
function createPublishEncoders(
  broadcast: Publish.Broadcast,
  capture: Publish.Video.Capture | null,
  microphone: MicrophoneHandle | null,
  audioEnabledInitially: boolean,
  encoderConfig: Publish.Video.EncoderProps['config']
): {
  audio: { enabled: Signal<boolean>; encoder: Publish.Audio.Encoder } | null;
  video: Publish.Video.Encoder | null;
} {
  let audio: { enabled: Signal<boolean>; encoder: Publish.Audio.Encoder } | null = null;
  try {
    if (microphone) {
      const audioEnabled = new Signal(audioEnabledInitially);
      audio = {
        enabled: audioEnabled,
        encoder: new Publish.Audio.Encoder(AUDIO_TRACK_NAME, {
          broadcast,
          enabled: audioEnabled,
          source: microphone.source,
        }),
      };
    }

    const video = capture
      ? new Publish.Video.Encoder(VIDEO_TRACK_NAME, {
          broadcast,
          capture,
          enabled: true,
          config: encoderConfig,
        })
      : null;

    return { audio, video };
  } catch (e) {
    audio?.encoder.close();
    capture?.close();
    broadcast.close();
    throw e;
  }
}

async function setupPublishPath(
  healthEffect: Effect,
  connection: Moq.Connection.Reload,
  inputBroadcast: string,
  needsAudio: boolean,
  needsVideo: boolean,
  videoSourceType: VideoSourceType,
  tracks: PublishTrackConfig[],
  set: StateSetter,
  abortSignal?: AbortSignal
): Promise<{
  microphone: MicrophoneHandle | null;
  camera: CameraHandle | null;
  screen: ScreenHandle | null;
  publish: PublishHandle;
}> {
  // Resolve per-track media hints for the primary video track.
  const videoTrack = tracks.find((t) => t.kind === 'video') ?? null;
  const { encoderConfig, constraints: videoConstraints } = buildVideoEncoderConfig(videoTrack);

  const { microphone, camera, screen } = await setupMediaSources(
    healthEffect,
    needsAudio,
    needsVideo,
    videoSourceType,
    set,
    abortSignal,
    videoConstraints
  );

  logger.info('Step 5: Creating publish broadcast');
  // When both audio and video are needed, start audio disabled and enable it
  // after the video encoder is ready.  This ensures both tracks begin MoQ
  // publishing at the same time, preventing the ~0.7s A/V desync from the
  // VP9 encoder's slower startup.
  const deferAudioUntilVideo = needsAudio && needsVideo;

  const capture = needsVideo
    ? createVideoCapture(healthEffect, videoSourceType === 'screen' ? screen : null, camera)
    : null;

  const broadcast = new Publish.Broadcast({
    connection: connection.established,
    enabled: true,
    name: Moq.Path.from(inputBroadcast),
    display: capture?.out.display,
  });

  const { audio, video } = createPublishEncoders(
    broadcast,
    capture,
    needsAudio ? microphone : null,
    !deferAudioUntilVideo,
    encoderConfig
  );

  const publish = new PublishHandle({ broadcast, capture, video, audio });

  // Wait for the video encoder to produce a catalog entry before returning.
  if (needsVideo && video) {
    logger.info('Step 5b: Waiting for video catalog...');
    try {
      await waitForSignalValue(
        video.out.catalog,
        (v) => v !== undefined,
        10_000,
        'Video encoder failed to initialize',
        abortSignal
      );
    } catch (e) {
      publish.close();
      shutdownMediaSource(screen);
      shutdownMediaSource(camera);
      shutdownMediaSource(microphone);
      throw e;
    }
    logger.info('Step 5b: Video catalog ready');
    // Now that video is publishing, enable audio so both tracks start
    // at the same time on the server side.
    if (deferAudioUntilVideo && audio) {
      audio.enabled.set(true);
      logger.info('Step 5c: Audio enabled (deferred until video ready)');
    }
  }

  return { microphone, camera, screen, publish };
}

/** Create a video capture source for a secondary broadcast and wait for device readiness. */
async function createSecondaryVideoSource(
  sourceType: 'camera' | 'screen',
  abortSignal?: AbortSignal,
  videoConstraints?: { width?: number; height?: number }
): Promise<{
  camera: CameraHandle | null;
  screen: ScreenHandle | null;
}> {
  if (sourceType === 'screen') {
    logger.info(
      'Creating secondary screen capture source — the OS may show an additional picker dialog'
    );
    const screen = new ScreenHandle(videoConstraints);
    if (!screen.source.peek()?.video) {
      try {
        await waitForSignalValue(
          screen.source,
          (v) => v?.video !== undefined,
          30_000,
          'Secondary screen capture not available',
          abortSignal
        );
      } catch (e) {
        shutdownMediaSource(screen);
        throw e;
      }
    }
    return { camera: null, screen };
  }

  const camera = new CameraHandle(videoConstraints);
  if (!camera.source.peek()) {
    try {
      await waitForSignalValue(
        camera.source,
        (v) => v !== undefined,
        15_000,
        'Secondary camera not available',
        abortSignal
      );
    } catch (e) {
      shutdownMediaSource(camera);
      throw e;
    }
  }
  return { camera, screen: null };
}

/** Analyze secondary broadcast tracks and return video source info + warnings.
 *  Pure function — no side effects, no logger calls. Warnings are collected
 *  for the caller to log. */
export function analyzeSecondaryBroadcastTracks(
  broadcastName: string,
  broadcastTracks: PublishTrackConfig[]
): {
  needsVideo: boolean;
  videoSourceType: VideoSourceType;
  warnings: string[];
} {
  const warnings: string[] = [];

  const hasAudio = broadcastTracks.some((t) => t.kind === 'audio');
  if (hasAudio) {
    warnings.push(
      `Secondary broadcast '${broadcastName}' has audio tracks which are not yet supported; ` +
        'audio will be silently dropped'
    );
  }

  const videoTracks = broadcastTracks.filter((t) => t.kind === 'video');
  const needsVideo = videoTracks.length > 0;
  if (videoTracks.length > 1) {
    warnings.push(
      `Secondary broadcast '${broadcastName}' has ${videoTracks.length} video tracks ` +
        'but only the first is used; additional video tracks are ignored'
    );
  }
  const videoSourceType: VideoSourceType =
    videoTracks[0]?.source === 'screen' ? 'screen' : 'camera';

  return { needsVideo, videoSourceType, warnings };
}

/** Filter tracks that belong to a secondary broadcast.
 *  Tracks without an explicit `broadcast` field default to `primaryBroadcast`. */
export function filterSecondaryTracks(
  tracks: PublishTrackConfig[],
  primaryBroadcast: string,
  secondaryBroadcast: string
): PublishTrackConfig[] {
  return tracks.filter((t) => (t.broadcast ?? primaryBroadcast) === secondaryBroadcast);
}

async function setupSecondaryPublishPath(
  healthEffect: Effect,
  connection: Moq.Connection.Reload,
  broadcastName: string,
  broadcastTracks: PublishTrackConfig[],
  abortSignal?: AbortSignal
): Promise<{
  secondaryPublish: PublishHandle;
  secondaryCamera: CameraHandle | null;
  secondaryScreen: ScreenHandle | null;
}> {
  const { needsVideo, videoSourceType, warnings } = analyzeSecondaryBroadcastTracks(
    broadcastName,
    broadcastTracks
  );
  for (const w of warnings) logger.warn(w);

  // Resolve per-track media hints for the secondary video track.
  const videoTrack = broadcastTracks.find((t) => t.kind === 'video') ?? null;
  const { encoderConfig, constraints: videoConstraints } = buildVideoEncoderConfig(videoTrack);

  let secondaryCamera: CameraHandle | null = null;
  let secondaryScreen: ScreenHandle | null = null;

  if (needsVideo) {
    const sources = await createSecondaryVideoSource(
      videoSourceType,
      abortSignal,
      videoConstraints
    );
    secondaryCamera = sources.camera;
    secondaryScreen = sources.screen;
  }

  logger.info(`Setting up secondary publish broadcast '${broadcastName}'`);
  const capture = needsVideo
    ? createVideoCapture(healthEffect, secondaryScreen, secondaryCamera)
    : null;

  const broadcast = new Publish.Broadcast({
    connection: connection.established,
    enabled: true,
    name: Moq.Path.from(broadcastName),
    display: capture?.out.display,
  });

  let video: Publish.Video.Encoder | null = null;
  if (capture) {
    video = new Publish.Video.Encoder(VIDEO_TRACK_NAME, {
      broadcast,
      capture,
      enabled: true,
      config: encoderConfig,
    });
  }

  const secondaryPublish = new PublishHandle({ broadcast, capture, video });

  if (needsVideo && video) {
    logger.info(`Waiting for secondary broadcast '${broadcastName}' video catalog...`);
    try {
      await waitForSignalValue(
        video.out.catalog,
        (v) => v !== undefined,
        10_000,
        `Secondary broadcast '${broadcastName}' video encoder failed to initialize`,
        abortSignal
      );
    } catch (e) {
      secondaryPublish.close();
      shutdownMediaSource(secondaryScreen);
      shutdownMediaSource(secondaryCamera);
      throw e;
    }
    logger.info(`Secondary broadcast '${broadcastName}' video catalog ready`);
  }

  return { secondaryPublish, secondaryCamera, secondaryScreen };
}

function schedulePostConnectWarnings(
  decision: Extract<ConnectDecision, { ok: true }>,
  attempt: ConnectAttempt,
  get: () => ConnectableState & { outputBroadcast: string },
  set: StateSetter
): void {
  if (!attempt.healthEffect) return;

  if (decision.shouldWatch && attempt.watch) {
    const watchRef = attempt.watch;
    setTimeout(() => {
      if (get().status !== 'connected') return;
      if (watchRef.out.status.peek() !== 'live') {
        set({
          errorMessage: `Connected to relay, but output broadcast "${get().outputBroadcast}" is not live yet.`,
        });
      }
    }, 10_000);
  }

  if (decision.shouldPublish && attempt.microphone) {
    const microphoneRef = attempt.microphone;

    let wasEverReady = Boolean(microphoneRef.source.peek()) || get().micStatus === 'ready';
    attempt.healthEffect.subscribe(microphoneRef.source, (value) => {
      if (value) wasEverReady = true;
    });

    setTimeout(() => {
      if (get().status !== 'connected') return;
      if (wasEverReady) return;
      set({
        micStatus: 'error',
        errorMessage:
          'Connected to relay, but microphone is not available. Check browser permissions and selected input device.',
      });
    }, 10_000);
  }

  if (decision.shouldPublish && attempt.camera) {
    const cameraRef = attempt.camera;

    let wasEverReady = Boolean(cameraRef.source.peek()) || get().cameraStatus === 'ready';
    attempt.healthEffect.subscribe(cameraRef.source, (value) => {
      if (value) wasEverReady = true;
    });

    setTimeout(() => {
      if (get().status !== 'connected') return;
      if (wasEverReady) return;
      set({
        cameraStatus: 'error',
        errorMessage:
          'Connected to relay, but camera is not available. Check browser permissions and selected input device.',
      });
    }, 10_000);
  }

  if (decision.shouldPublish && attempt.screen) {
    const screenRef = attempt.screen;

    let wasEverReady = Boolean(screenRef.source.peek()?.video);
    attempt.healthEffect.subscribe(screenRef.source, (value) => {
      if (value?.video) wasEverReady = true;
    });

    setTimeout(() => {
      if (get().status !== 'connected') return;
      if (wasEverReady) return;
      set({
        cameraStatus: 'error',
        errorMessage:
          'Connected to relay, but screen capture is not available. The user may have stopped sharing.',
      });
    }, 10_000);
  }

  if (decision.shouldPublish && attempt.secondaryCamera) {
    const cameraRef = attempt.secondaryCamera;

    let wasEverReady = Boolean(cameraRef.source.peek()) || get().secondaryCameraStatus === 'ready';
    attempt.healthEffect.subscribe(cameraRef.source, (value) => {
      if (value) wasEverReady = true;
    });

    setTimeout(() => {
      if (get().status !== 'connected') return;
      if (wasEverReady) return;
      set({
        secondaryCameraStatus: 'error',
        errorMessage:
          'Connected to relay, but secondary camera is not available. Check browser permissions.',
      });
    }, 10_000);
  }

  if (decision.shouldPublish && attempt.secondaryScreen) {
    const screenRef = attempt.secondaryScreen;

    let wasEverReady = Boolean(screenRef.source.peek()?.video);
    attempt.healthEffect.subscribe(screenRef.source, (value) => {
      if (value?.video) wasEverReady = true;
    });

    setTimeout(() => {
      if (get().status !== 'connected') return;
      if (wasEverReady) return;
      set({
        secondaryCameraStatus: 'error',
        errorMessage:
          'Connected to relay, but secondary screen capture is not available. The user may have stopped sharing.',
      });
    }, 10_000);
  }
}

function applyWatchResult(
  attempt: ConnectAttempt,
  result: ReturnType<typeof setupWatchPath>
): void {
  attempt.watch = result.watch;
  attempt.watchSync = result.watchSync;
  attempt.audioSource = result.audioSource;
  attempt.audioDecoder = result.audioDecoder;
  attempt.audioEmitter = result.audioEmitter;
  attempt.videoSource = result.videoSource;
  attempt.videoDecoder = result.videoDecoder;
  attempt.videoRenderer = result.videoRenderer;
}

function applyPublishResult(
  attempt: ConnectAttempt,
  result: Awaited<ReturnType<typeof setupPublishPath>>
): void {
  attempt.microphone = result.microphone;
  attempt.camera = result.camera;
  attempt.screen = result.screen;
  attempt.publish = result.publish;
}

function applySecondaryPublishResult(
  attempt: ConnectAttempt,
  result: Awaited<ReturnType<typeof setupSecondaryPublishPath>>
): void {
  attempt.secondaryPublish = result.secondaryPublish;
  attempt.secondaryCamera = result.secondaryCamera;
  attempt.secondaryScreen = result.secondaryScreen;
}

/** Set up secondary broadcast if the pipeline declares more than one.
 *  UI limitation: at most 2 broadcasts total (1 primary + 1 secondary).
 *  The Rust backend (`MoqPeerNode`) supports N broadcasts, but the UI
 *  currently only wires a single secondary.  Extending to N would require
 *  dynamic `ConnectAttempt` fields and per-broadcast cleanup tracking. */
async function setupSecondaryBroadcastIfNeeded(
  attempt: ConnectAttempt,
  state: ConnectableState,
  abortSignal: AbortSignal
): Promise<void> {
  if (state.publishBroadcasts.length <= 1) return;

  if (state.publishBroadcasts.length > 2) {
    logger.warn(
      `Pipeline declares ${state.publishBroadcasts.length} broadcasts but only 2 are supported; ` +
        `ignoring: ${state.publishBroadcasts.slice(2).join(', ')}`
    );
  }
  const primaryBroadcast = state.publishBroadcasts[0];
  const secondaryName = state.publishBroadcasts[1];
  const secondaryTracks = filterSecondaryTracks(state.tracks, primaryBroadcast, secondaryName);

  if (secondaryTracks.length > 0) {
    // Secondary setup runs after the primary because both share the same
    // connection: if the secondary fails, the primary resources must already
    // be on `attempt` so cleanupConnectAttempt can tear them down.
    // setupSecondaryPublishPath owns cleanup of its resources on failure;
    // on success, ownership transfers to `attempt` via applySecondaryPublishResult,
    // and the outer catch block handles cleanup through cleanupConnectAttempt.
    applySecondaryPublishResult(
      attempt,
      await setupSecondaryPublishPath(
        attempt.healthEffect!,
        attempt.connection!,
        secondaryName,
        secondaryTracks,
        abortSignal
      )
    );
  } else {
    logger.warn(`Secondary broadcast '${secondaryName}' declared but no tracks matched; skipping`);
  }
}

/** Create the MoQ connection and wire up the health-status sync effect. */
function createConnectionAndHealth(
  serverUrl: string,
  moqToken: string,
  get: () => ConnectableState,
  set: StateSetter
): { connection: Moq.Connection.Reload; healthEffect: Effect } {
  logger.info('Step 1: Creating connection to relay server');
  const url = new URL(serverUrl);
  const jwt = moqToken.trim();
  if (jwt) {
    url.searchParams.set('jwt', jwt);
  }

  const connection = new Moq.Connection.Reload({ url, enabled: true });
  const healthEffect = new Effect();
  setupConnectionStatusSync(healthEffect, connection, get, set);
  return { connection, healthEffect };
}

/** Wait for relay connection, optionally wait for broadcast announcement, then set up watch. */
async function connectWatchPath(
  attempt: ConnectAttempt,
  state: ConnectableState,
  decision: Extract<ConnectDecision, { ok: true }>,
  set: StateSetter,
  abortSignal: AbortSignal
): Promise<void> {
  set({ connectingStep: 'relay' });
  await waitForSignalValue(
    attempt.connection!.established,
    (value) => value !== undefined,
    12_000,
    'Timed out connecting to MoQ gateway.',
    abortSignal
  );

  if (decision.shouldWatch && state.outputBroadcast) {
    // When publishing to an external relay, the skit pipeline needs time to
    // discover input tracks, build the graph, and start publishing output.
    // Wait for the output broadcast to be announced on the relay before
    // subscribing, otherwise the catalog subscribe gets RESET_STREAM.
    // In gateway mode the skit server manages the peer connection directly,
    // so no announcement polling is needed.
    if (decision.shouldPublish && state.isExternalRelay) {
      set({ connectingStep: 'pipeline' });
      await waitForBroadcastAnnouncement(
        attempt.connection!,
        state.outputBroadcast,
        15_000,
        abortSignal
      );
    }

    applyWatchResult(
      attempt,
      setupWatchPath(
        attempt.healthEffect!,
        attempt.connection!,
        state.outputBroadcast,
        state.pipelineOutputsAudio,
        state.pipelineOutputsVideo,
        set
      )
    );
  }
}

/** Core connection logic extracted from the store for reduced complexity. */
export async function performConnect(
  state: ConnectableState,
  decision: Extract<ConnectDecision, { ok: true }>,
  get: () => ConnectableState & { outputBroadcast: string },
  set: StateSetter,
  abortSignal: AbortSignal
): Promise<boolean> {
  const attempt: ConnectAttempt = { ...NULL_MOQ_REFS };

  try {
    if (abortSignal.aborted) throw new DOMException('Aborted', 'AbortError');

    const { connection, healthEffect } = createConnectionAndHealth(
      decision.trimmedServerUrl,
      state.moqToken,
      get,
      set
    );
    attempt.connection = connection;
    attempt.healthEffect = healthEffect;

    // Set up publish BEFORE watch. For external relay pipelines (pub/sub),
    // the skit pipeline needs input data before it can publish output.
    // If we watch first, the subscribe to output/catalog.json fails with
    // RESET_STREAM because skit hasn't started publishing yet.
    if (decision.shouldPublish) {
      validateTrackCodecs(state.tracks);
      set({ connectingStep: 'devices' });

      const primaryBroadcast = state.publishBroadcasts[0] ?? state.inputBroadcast;
      const primaryTracks = state.tracks.filter(
        (t) => (t.broadcast ?? primaryBroadcast) === primaryBroadcast
      );

      applyPublishResult(
        attempt,
        await setupPublishPath(
          attempt.healthEffect,
          attempt.connection,
          state.inputBroadcast,
          state.pipelineNeedsAudio,
          state.pipelineNeedsVideo,
          state.videoSourceType,
          primaryTracks,
          set,
          abortSignal
        )
      );

      // Secondary broadcast (multi-broadcast mode).
      await setupSecondaryBroadcastIfNeeded(attempt, state, abortSignal);
    }

    await connectWatchPath(attempt, state, decision, set, abortSignal);

    // If aborted between the last await and now, discard this attempt
    // so we don't overwrite a newer connect's state.
    if (abortSignal.aborted) {
      cleanupConnectAttempt(attempt);
      return false;
    }

    schedulePostConnectWarnings(decision, attempt, get, set);

    set({
      ...attempt,
      status: 'connected',
      connectingStep: '',
      isMicEnabled: decision.shouldPublish && state.pipelineNeedsAudio,
      isCameraEnabled: decision.shouldPublish && state.pipelineNeedsVideo,
      isSecondaryCameraEnabled: Boolean(attempt.secondaryCamera ?? attempt.secondaryScreen),
      secondaryCameraStatus:
        (attempt.secondaryCamera ?? attempt.secondaryScreen) ? 'ready' : 'disabled',
    });

    const modes = [];
    if (decision.shouldWatch) modes.push('watching');
    if (decision.shouldPublish) modes.push('publishing');
    logger.info(`Connection established: ${modes.join(' and ')}`);
    return true;
  } catch (error) {
    cleanupConnectAttempt(attempt);

    // If this attempt was aborted (superseded by disconnect or a newer
    // connect), silently discard — don't overwrite the store.
    if (abortSignal.aborted) {
      return false;
    }

    logger.error('Connection failed:', error);
    set({
      status: 'disconnected',
      connectingStep: '',
      watchStatus: 'disabled',
      micStatus: 'disabled',
      cameraStatus: 'disabled',
      isSecondaryCameraEnabled: false,
      secondaryCameraStatus: 'disabled',
      errorMessage: formatConnectError(error),
      ...NULL_MOQ_REFS,
    });
    return false;
  }
}
