// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/** Helpers for streamStore — connection lifecycle, teardown, and health warnings. */

import * as Hang from '@moq/hang';
import * as Publish from '@moq/publish';
import type { Getter } from '@moq/signals';
import { Effect } from '@moq/signals';
import * as Watch from '@moq/watch';

import type { CameraStatus, ConnectionStatus, MicStatus, WatchStatus } from './streamStore';
import { getLogger } from '../utils/logger';

const logger = getLogger('streamStore');

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
  publish: Publish.Broadcast | null;
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
  status: ConnectionStatus;
  errorMessage: string;
  isMicEnabled: boolean;
  isCameraEnabled: boolean;
  micStatus: MicStatus;
  cameraStatus: CameraStatus;
  watchStatus: WatchStatus;
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
  healthEffect: null,
} as const;

export function waitForSignalValue<T>(
  signal: Getter<T>,
  predicate: (value: T) => boolean,
  timeoutMs: number,
  timeoutMessage: string
): Promise<T> {
  const initial = signal.peek();
  if (predicate(initial)) {
    return Promise.resolve(initial);
  }

  return new Promise((resolve, reject) => {
    let dispose: () => void = () => {};
    const timeoutId = setTimeout(() => {
      dispose();
      reject(new Error(timeoutMessage));
    }, timeoutMs);

    dispose = signal.subscribe((value) => {
      if (predicate(value)) {
        clearTimeout(timeoutId);
        dispose();
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
  timeoutMs = 15_000
): Promise<void> {
  const conn = connection.established.peek();
  if (!conn) return;

  logger.info(`Waiting for broadcast '${broadcastName}' to be announced...`);

  const announcements = conn.announced();
  const deadline = Date.now() + timeoutMs;

  try {
    while (Date.now() < deadline) {
      const remaining = deadline - Date.now();
      const entry = await Promise.race([
        announcements.next(),
        new Promise<null>((r) => setTimeout(() => r(null), remaining)),
      ]);
      if (!entry) break; // timeout
      if (entry.active && entry.path.toString() === broadcastName) {
        logger.info(`Broadcast '${broadcastName}' announced`);
        return;
      }
    }
    logger.warn(
      `Broadcast '${broadcastName}' not announced within ${timeoutMs}ms, proceeding anyway`
    );
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
  source: Publish.Source.Microphone | Publish.Source.Camera | null
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

async function setupPublishPath(
  healthEffect: Effect,
  connection: Hang.Moq.Connection.Reload,
  inputBroadcast: string,
  needsAudio: boolean,
  needsVideo: boolean,
  set: StateSetter
): Promise<{
  microphone: Publish.Source.Microphone | null;
  camera: Publish.Source.Camera | null;
  publish: Publish.Broadcast;
}> {
  let microphone: Publish.Source.Microphone | null = null;
  let camera: Publish.Source.Camera | null = null;

  if (needsAudio) {
    logger.info('Step 4: Creating microphone source');
    microphone = new Publish.Source.Microphone({ enabled: true });

    set({ micStatus: microphone.source.peek() ? 'ready' : 'requesting' });
    healthEffect.subscribe(microphone.source, (value) => {
      set({ micStatus: value ? 'ready' : 'requesting' });
    });
  }

  if (needsVideo) {
    logger.info('Step 4b: Creating camera source');
    camera = new Publish.Source.Camera({ enabled: true });

    set({ cameraStatus: camera.source.peek() ? 'ready' : 'requesting' });
    healthEffect.subscribe(camera.source, (value) => {
      set({ cameraStatus: value ? 'ready' : 'requesting' });
    });

    // Wait for the camera to become available before creating the broadcast.
    // The MoQ catalog is published as soon as the broadcast is created;
    // if the camera isn't ready yet the catalog will only list audio tracks
    // and downstream subscribers (e.g. skit's MoqPullNode) won't discover
    // the video track because they only read the catalog once at startup.
    if (!camera.source.peek()) {
      logger.info('Step 4c: Waiting for camera permission...');
      await waitForSignalValue(
        camera.source,
        (v) => v !== undefined,
        15_000,
        'Camera not available'
      );
      logger.info('Step 4c: Camera ready');
    }
  }

  logger.info('Step 5: Creating publish broadcast');

  const broadcastConfig: ConstructorParameters<typeof Publish.Broadcast>[0] = {
    connection: connection.established,
    enabled: true,
    name: Publish.Lite.Path.from(inputBroadcast),
  };
  if (needsAudio && microphone) {
    broadcastConfig.audio = {
      enabled: true,
      source: microphone.source,
    };
  }
  if (needsVideo && camera) {
    broadcastConfig.video = {
      source: camera.source,
      hd: { enabled: true, config: { codec: 'vp09' } },
    };
  }

  const publish = new Publish.Broadcast(broadcastConfig);

  // When video is enabled, wait for the video encoder pipeline to produce a
  // catalog entry before returning. The encoder chain is async: camera frame →
  // dimensions → codec probe → config → catalog.  If we return before this
  // completes, the broadcast catalog will omit video and downstream subscribers
  // (which only read the catalog once) will never discover the video track.
  if (needsVideo) {
    logger.info('Step 5b: Waiting for video catalog...');
    await waitForSignalValue(
      publish.video.catalog,
      (v) => v !== undefined,
      10_000,
      'Video encoder failed to initialize'
    );
    logger.info('Step 5b: Video catalog ready');
  }

  return { microphone, camera, publish };
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
  attempt.publish = result.publish;
}

/** Core connection logic extracted from the store for reduced complexity. */
export async function performConnect(
  state: ConnectableState,
  decision: Extract<ConnectDecision, { ok: true }>,
  get: () => ConnectableState & { outputBroadcast: string },
  set: StateSetter
): Promise<boolean> {
  const attempt: ConnectAttempt = {
    connection: null,
    healthEffect: null,
    watch: null,
    watchSync: null,
    audioSource: null,
    audioDecoder: null,
    audioEmitter: null,
    videoSource: null,
    videoDecoder: null,
    videoRenderer: null,
    microphone: null,
    camera: null,
    publish: null,
  };

  try {
    logger.info('Step 1: Creating connection to relay server');
    const url = new URL(decision.trimmedServerUrl);
    const jwt = state.moqToken.trim();
    if (jwt) {
      url.searchParams.set('jwt', jwt);
    }

    attempt.connection = new Hang.Moq.Connection.Reload({ url, enabled: true });
    attempt.healthEffect = new Effect();
    setupConnectionStatusSync(attempt.healthEffect, attempt.connection, get, set);

    // Set up publish BEFORE watch. For external relay pipelines (pub/sub),
    // the skit pipeline needs input data before it can publish output.
    // If we watch first, the subscribe to output/catalog.json fails with
    // RESET_STREAM because skit hasn't started publishing yet.
    if (decision.shouldPublish) {
      applyPublishResult(
        attempt,
        await setupPublishPath(
          attempt.healthEffect,
          attempt.connection,
          state.inputBroadcast,
          state.pipelineNeedsAudio,
          state.pipelineNeedsVideo,
          set
        )
      );
    }

    await waitForSignalValue(
      attempt.connection.established,
      (value) => value !== undefined,
      12_000,
      'Timed out connecting to MoQ gateway.'
    );

    if (decision.shouldWatch) {
      // When publishing to an external relay, the skit pipeline needs time to
      // discover input tracks, build the graph, and start publishing output.
      // Wait for the output broadcast to be announced on the relay before
      // subscribing, otherwise the catalog subscribe gets RESET_STREAM.
      if (decision.shouldPublish) {
        await waitForBroadcastAnnouncement(attempt.connection, state.outputBroadcast);
      }

      applyWatchResult(
        attempt,
        setupWatchPath(
          attempt.healthEffect,
          attempt.connection,
          state.outputBroadcast,
          state.pipelineOutputsAudio,
          state.pipelineOutputsVideo,
          set
        )
      );
    }

    schedulePostConnectWarnings(decision, attempt, get, set);

    set({
      ...attempt,
      status: 'connected',
      isMicEnabled: decision.shouldPublish && state.pipelineNeedsAudio,
      isCameraEnabled: decision.shouldPublish && state.pipelineNeedsVideo,
    });

    const modes = [];
    if (decision.shouldWatch) modes.push('watching');
    if (decision.shouldPublish) modes.push('publishing');
    logger.info(`Connection established: ${modes.join(' and ')}`);
    return true;
  } catch (error) {
    logger.error('Connection failed:', error);
    cleanupConnectAttempt(attempt);

    set({
      status: 'disconnected',
      watchStatus: 'disabled',
      micStatus: 'disabled',
      cameraStatus: 'disabled',
      errorMessage: formatConnectError(error),
      ...NULL_MOQ_REFS,
    });
    return false;
  }
}
