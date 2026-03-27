// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/** Helpers for streamStore — connection lifecycle, teardown, and health warnings. */

import * as Hang from '@moq/hang';
import * as Publish from '@moq/publish';
import type { Getter } from '@moq/signals';
import { Effect, Signal } from '@moq/signals';
import * as Watch from '@moq/watch';

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
const SUPPORTED_VIDEO_CODECS = ['vp9'];
const SUPPORTED_AUDIO_CODECS = ['opus'];

/** Map user-facing codec name to WebCodecs codec string. */
function mapCodecToWebCodecs(codec: string): string {
  switch (codec) {
    case 'vp9':
      return 'vp09';
    default:
      return codec;
  }
}

/**
 * Build `@moq/publish` encoder config and capture constraints from per-track
 * media hints.  Pure function — no side effects.
 *
 * @param track - The track config containing optional width/height/codec/max_bitrate.
 * @returns encoderConfig for `Publish.Broadcast` and optional capture constraints.
 */
export function buildVideoEncoderConfig(track?: PublishTrackConfig | null): {
  encoderConfig: { codec: string; maxPixels?: number; maxBitrate?: number };
  constraints?: { width?: number; height?: number };
} {
  const codec = mapCodecToWebCodecs(track?.codec ?? 'vp9');
  const encoderConfig: { codec: string; maxPixels?: number; maxBitrate?: number } = { codec };

  const width = track?.width ?? undefined;
  const height = track?.height ?? undefined;

  if (width != null && height != null) {
    encoderConfig.maxPixels = width * height;
  }
  if (track?.max_bitrate != null) {
    // Convert kbps → bps for the encoder.
    encoderConfig.maxBitrate = track.max_bitrate * 1000;
  }

  const hasConstraints = width != null || height != null;
  const constraints = hasConstraints ? { width: width, height: height } : undefined;

  return { encoderConfig, constraints };
}

/**
 * Emit `logger.warn` for each track whose codec is not in the recognized set.
 * Pure validation — does not throw or modify state.
 */
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

export type ConnectAttempt = {
  connection: Hang.Moq.Connection.Reload | null;
  healthEffect: Effect | null;
  watch: Watch.Broadcast | null;
  watchSync: Watch.Sync | null;
  audioSource: Watch.Audio.Source | null;
  audioDecoder: Watch.Audio.Decoder | null;
  audioEmitter: Watch.Audio.Emitter | null;
  videoSource: Watch.Video.Source | null;
  videoDecoder: Watch.Video.Decoder | null;
  videoRenderer: Watch.Video.Renderer | null;
  microphone: Publish.Source.Microphone | null;
  camera: Publish.Source.Camera | null;
  screen: Publish.Source.Screen | null;
  publish: Publish.Broadcast | null;
  /** Secondary publish broadcast for multi-broadcast mode (e.g. camera PiP). */
  secondaryPublish: Publish.Broadcast | null;
  /** Secondary camera source for multi-broadcast mode. */
  secondaryCamera: Publish.Source.Camera | null;
  /** Secondary screen source for multi-broadcast mode. */
  secondaryScreen: Publish.Source.Screen | null;
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

/**
 * Waits for a specific broadcast to be announced on the MoQ relay.
 * Used when publishing to an external relay: the skit pipeline needs time to
 * discover input, build the graph, and start publishing output. Polling for
 * the announcement avoids a fixed delay.
 */
async function waitForBroadcastAnnouncement(
  connection: Hang.Moq.Connection.Reload,
  broadcastName: string,
  timeoutMs = 15_000,
  abortSignal?: AbortSignal
): Promise<void> {
  if (abortSignal?.aborted) throw new DOMException('Aborted', 'AbortError');
  const conn = connection.established.peek();
  if (!conn) return;
  logger.info(`Waiting for broadcast '${broadcastName}' announcement...`);
  const announcements = conn.announced();
  const deadline = Date.now() + timeoutMs;
  try {
    // Build a promise that rejects when the abort signal fires so the
    // Promise.race below reacts immediately instead of polling.
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
  // NOTE: Session mode no longer implicitly enables publishing.  Publishing is
  // now driven entirely by `enablePublish` (which the session setup sets based
  // on whether the pipeline needs client-side media inputs).  This was a
  // deliberate change from the old behaviour where session mode always published.
  const shouldPublish = state.enablePublish;

  return { ok: true, trimmedServerUrl, shouldWatch, shouldPublish };
}

export function formatConnectError(error: unknown): string {
  return error instanceof Error
    ? `Connection failed: ${error.message}`
    : 'Failed to connect to MoQ server. Please check your connection and try again.';
}

/** Shut down a media source that may expose `.close()` or only `.enabled`. */
function shutdownMediaSource(
  source: Publish.Source.Microphone | Publish.Source.Camera | Publish.Source.Screen | null
): void {
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
  connection: Hang.Moq.Connection.Reload,
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
  connection: Hang.Moq.Connection.Reload,
  outputBroadcast: string,
  outputsAudio: boolean,
  outputsVideo: boolean,
  set: StateSetter
): {
  watch: Watch.Broadcast;
  watchSync: Watch.Sync;
  audioSource: Watch.Audio.Source | null;
  audioDecoder: Watch.Audio.Decoder | null;
  audioEmitter: Watch.Audio.Emitter | null;
  videoSource: Watch.Video.Source | null;
  videoDecoder: Watch.Video.Decoder | null;
  videoRenderer: Watch.Video.Renderer | null;
} {
  logger.info('Step 2: Creating watch broadcast (subscribe FIRST)');
  const watch = new Watch.Broadcast({
    connection: connection.established,
    enabled: true,
    name: Watch.Lite.Path.from(outputBroadcast),
  });

  const watchSync = new Watch.Sync();

  let audioSource: Watch.Audio.Source | null = null;
  let audioDecoder: Watch.Audio.Decoder | null = null;
  let audioEmitter: Watch.Audio.Emitter | null = null;

  if (outputsAudio) {
    logger.info('Step 3: Creating audio source/decoder/emitter');
    audioSource = new Watch.Audio.Source(watchSync, { broadcast: watch });
    audioDecoder = new Watch.Audio.Decoder(audioSource);
    audioEmitter = new Watch.Audio.Emitter(audioDecoder, {
      muted: false,
      volume: 0.5,
    });
  }

  let videoSource: Watch.Video.Source | null = null;
  let videoDecoder: Watch.Video.Decoder | null = null;
  let videoRenderer: Watch.Video.Renderer | null = null;

  if (outputsVideo) {
    logger.info('Step 3b: Creating video source/decoder/renderer');
    videoSource = new Watch.Video.Source(watchSync, { broadcast: watch });
    videoDecoder = new Watch.Video.Decoder(videoSource);
    videoRenderer = new Watch.Video.Renderer(videoDecoder);
  }

  set({ watchStatus: watch.status.peek() });
  healthEffect.subscribe(watch.status, (value) => {
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

/** Create media sources and wait for device permission if needed. */
async function setupMediaSources(
  healthEffect: Effect,
  needsAudio: boolean,
  needsVideo: boolean,
  videoSourceType: VideoSourceType,
  set: StateSetter,
  abortSignal?: AbortSignal,
  videoConstraints?: { width?: number; height?: number }
): Promise<{
  microphone: Publish.Source.Microphone | null;
  camera: Publish.Source.Camera | null;
  screen: Publish.Source.Screen | null;
}> {
  let microphone: Publish.Source.Microphone | null = null;
  let camera: Publish.Source.Camera | null = null;
  let screen: Publish.Source.Screen | null = null;
  if (needsAudio) {
    microphone = new Publish.Source.Microphone({ enabled: true });
    set({ micStatus: microphone.source.peek() ? 'ready' : 'requesting' });
    healthEffect.subscribe(microphone.source, (v) =>
      set({ micStatus: v ? 'ready' : 'requesting' })
    );
  }
  if (needsVideo) {
    if (videoSourceType === 'screen') {
      // Screen capture — the OS picker dialog may take a while, use 30s timeout.
      const screenProps: ConstructorParameters<typeof Publish.Source.Screen>[0] = { enabled: true };
      if (videoConstraints) {
        screenProps.video = videoConstraints;
      }
      screen = new Publish.Source.Screen(screenProps);
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
    } else {
      // Camera capture — standard 15s timeout for permission dialog.
      const cameraProps: ConstructorParameters<typeof Publish.Source.Camera>[0] = { enabled: true };
      if (videoConstraints) {
        cameraProps.constraints = videoConstraints;
      }
      camera = new Publish.Source.Camera(cameraProps);
      set({ cameraStatus: camera.source.peek() ? 'ready' : 'requesting' });
      healthEffect.subscribe(camera.source, (v) =>
        set({ cameraStatus: v ? 'ready' : 'requesting' })
      );
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
    }
  }
  return { microphone, camera, screen };
}

async function setupPublishPath(
  healthEffect: Effect,
  connection: Hang.Moq.Connection.Reload,
  inputBroadcast: string,
  needsAudio: boolean,
  needsVideo: boolean,
  videoSourceType: VideoSourceType,
  tracks: PublishTrackConfig[],
  set: StateSetter,
  abortSignal?: AbortSignal
): Promise<{
  microphone: Publish.Source.Microphone | null;
  camera: Publish.Source.Camera | null;
  screen: Publish.Source.Screen | null;
  publish: Publish.Broadcast;
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
  const broadcastConfig: ConstructorParameters<typeof Publish.Broadcast>[0] = {
    connection: connection.established,
    enabled: true,
    name: Publish.Lite.Path.from(inputBroadcast),
  };
  if (needsAudio && microphone) {
    broadcastConfig.audio = { enabled: true, source: microphone.source };
  }
  if (needsVideo) {
    if (videoSourceType === 'screen' && screen) {
      // Screen capture: derive a video-only signal from the composite
      // Screen.source signal ({ audio?, video? } | undefined).
      // System audio from screen capture is ignored — mic remains the
      // sole audio source.
      const videoOnlySignal = new Signal<Publish.Video.Source | undefined>(
        screen.source.peek()?.video
      );
      healthEffect.subscribe(screen.source, (v) => videoOnlySignal.set(v?.video));
      broadcastConfig.video = {
        source: videoOnlySignal,
        hd: { enabled: true, config: encoderConfig },
      };
    } else if (camera) {
      broadcastConfig.video = {
        source: camera.source,
        hd: { enabled: true, config: encoderConfig },
      };
    }
  }

  const publish = new Publish.Broadcast(broadcastConfig);

  // Wait for the video encoder to produce a catalog entry before returning.
  if (needsVideo) {
    logger.info('Step 5b: Waiting for video catalog...');
    try {
      await waitForSignalValue(
        publish.video.catalog,
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
  }

  return { microphone, camera, screen, publish };
}

/** Create a video capture source for a secondary broadcast and wait for device readiness. */
async function createSecondaryVideoSource(
  sourceType: 'camera' | 'screen',
  abortSignal?: AbortSignal,
  videoConstraints?: { width?: number; height?: number }
): Promise<{
  camera: Publish.Source.Camera | null;
  screen: Publish.Source.Screen | null;
}> {
  if (sourceType === 'screen') {
    logger.info(
      'Creating secondary screen capture source — the OS may show an additional picker dialog'
    );
    const screenProps: ConstructorParameters<typeof Publish.Source.Screen>[0] = { enabled: true };
    if (videoConstraints) {
      screenProps.video = videoConstraints;
    }
    const screen = new Publish.Source.Screen(screenProps);
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

  const cameraProps: ConstructorParameters<typeof Publish.Source.Camera>[0] = { enabled: true };
  if (videoConstraints) {
    cameraProps.constraints = videoConstraints;
  }
  const camera = new Publish.Source.Camera(cameraProps);
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
  connection: Hang.Moq.Connection.Reload,
  broadcastName: string,
  broadcastTracks: PublishTrackConfig[],
  abortSignal?: AbortSignal
): Promise<{
  secondaryPublish: Publish.Broadcast;
  secondaryCamera: Publish.Source.Camera | null;
  secondaryScreen: Publish.Source.Screen | null;
}> {
  const { needsVideo, videoSourceType, warnings } = analyzeSecondaryBroadcastTracks(
    broadcastName,
    broadcastTracks
  );
  for (const w of warnings) logger.warn(w);

  // Resolve per-track media hints for the secondary video track.
  const videoTrack = broadcastTracks.find((t) => t.kind === 'video') ?? null;
  const { encoderConfig, constraints: videoConstraints } = buildVideoEncoderConfig(videoTrack);

  let secondaryCamera: Publish.Source.Camera | null = null;
  let secondaryScreen: Publish.Source.Screen | null = null;

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
  const broadcastConfig: ConstructorParameters<typeof Publish.Broadcast>[0] = {
    connection: connection.established,
    enabled: true,
    name: Publish.Lite.Path.from(broadcastName),
  };

  if (needsVideo) {
    if (secondaryScreen) {
      const videoOnlySignal = new Signal<Publish.Video.Source | undefined>(
        secondaryScreen.source.peek()?.video
      );
      healthEffect.subscribe(secondaryScreen.source, (v) => videoOnlySignal.set(v?.video));
      broadcastConfig.video = {
        source: videoOnlySignal,
        hd: { enabled: true, config: encoderConfig },
      };
    } else if (secondaryCamera) {
      broadcastConfig.video = {
        source: secondaryCamera.source,
        hd: { enabled: true, config: encoderConfig },
      };
    }
  }

  const secondaryPublish = new Publish.Broadcast(broadcastConfig);

  if (needsVideo) {
    logger.info(`Waiting for secondary broadcast '${broadcastName}' video catalog...`);
    try {
      await waitForSignalValue(
        secondaryPublish.video.catalog,
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
      if (watchRef.status.peek() !== 'live') {
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

  // Camera warning mirrors the microphone warning above.  The guard
  // `attempt.camera` is null when `!needsVideo`, so this block is only
  // reached when the pipeline actually requested a camera source.
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

  // Screen source warning — analogous to the camera warning above.
  // Screen.source has shape { audio?, video? } | undefined, so we
  // check for the presence of the video track.
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
}

/** Apply watch-path results to the attempt object in a type-safe manner. */
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

/** Apply publish-path results to the attempt object in a type-safe manner. */
function applyPublishResult(
  attempt: ConnectAttempt,
  result: Awaited<ReturnType<typeof setupPublishPath>>
): void {
  attempt.microphone = result.microphone;
  attempt.camera = result.camera;
  attempt.screen = result.screen;
  attempt.publish = result.publish;
}

/** Apply secondary publish-path results to the attempt object. */
function applySecondaryPublishResult(
  attempt: ConnectAttempt,
  result: Awaited<ReturnType<typeof setupSecondaryPublishPath>>
): void {
  attempt.secondaryPublish = result.secondaryPublish;
  attempt.secondaryCamera = result.secondaryCamera;
  attempt.secondaryScreen = result.secondaryScreen;
}

/** Create the MoQ connection and wire up the health-status sync effect. */
function createConnectionAndHealth(
  serverUrl: string,
  moqToken: string,
  get: () => ConnectableState,
  set: StateSetter
): { connection: Hang.Moq.Connection.Reload; healthEffect: Effect } {
  logger.info('Step 1: Creating connection to relay server');
  const url = new URL(serverUrl);
  const jwt = moqToken.trim();
  if (jwt) {
    url.searchParams.set('jwt', jwt);
  }

  const connection = new Hang.Moq.Connection.Reload({ url, enabled: true });
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

  if (decision.shouldWatch) {
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

      // Filter tracks belonging to the primary broadcast for setupPublishPath.
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
      // UI limitation: at most 2 broadcasts total (1 primary + 1 secondary).
      // The Rust backend (`MoqPeerNode`) supports N broadcasts, but the UI
      // currently only wires a single secondary.  Extending to N would require
      // dynamic `ConnectAttempt` fields and per-broadcast cleanup tracking.
      if (state.publishBroadcasts.length > 1) {
        if (state.publishBroadcasts.length > 2) {
          logger.warn(
            `Pipeline declares ${state.publishBroadcasts.length} broadcasts but only 2 are supported; ` +
              `ignoring: ${state.publishBroadcasts.slice(2).join(', ')}`
          );
        }
        const primaryBroadcast = state.publishBroadcasts[0];
        const secondaryName = state.publishBroadcasts[1];
        const secondaryTracks = filterSecondaryTracks(
          state.tracks,
          primaryBroadcast,
          secondaryName
        );

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
          logger.warn(
            `Secondary broadcast '${secondaryName}' declared but no tracks matched; skipping`
          );
        }
      }
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
      errorMessage: formatConnectError(error),
      ...NULL_MOQ_REFS,
    });
    return false;
  }
}
