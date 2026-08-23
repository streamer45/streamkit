// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { act, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type { MoqSettingsActions } from '@/utils/moqPeerSettings';

import { useMoqYamlSync } from './useMoqYamlSync';

// useMoqYamlSync transitively imports the stream store, which pulls in the
// WebTransport-backed @moq/* libraries; stub them so the module loads in jsdom.
vi.mock('@moq/net', () => ({
  default: {},
  Connection: { Reload: vi.fn() },
  Path: { from: vi.fn() },
}));
vi.mock('@moq/hang', () => ({ default: {} }));
vi.mock('@moq/watch', () => ({ default: {}, Broadcast: vi.fn() }));
vi.mock('@moq/publish', () => ({ default: {}, Broadcast: vi.fn() }));
vi.mock('@moq/signals', () => ({ Effect: vi.fn() }));

function makeActions(): MoqSettingsActions {
  return {
    setServerUrl: vi.fn(),
    setInputBroadcast: vi.fn(),
    setOutputBroadcast: vi.fn(),
    setEnablePublish: vi.fn(),
    setEnableWatch: vi.fn(),
    setPipelineMediaTypes: vi.fn(),
    setPipelineOutputTypes: vi.fn(),
    setIsExternalRelay: vi.fn(),
    setVideoSourceType: vi.fn(),
    setTracks: vi.fn(),
    setMsePath: vi.fn(),
  };
}

const moqYaml = (broadcast: string, gateway = '/moq/test') =>
  ['client:', `  gateway_path: ${gateway}`, '  watch:', `    broadcast: ${broadcast}`].join('\n');

const nonMoqYaml = 'nodes:\n  colorbars:\n    kind: video::colorbars\n';

describe('useMoqYamlSync', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it('re-derives MoQ settings on a debounce after a direct YAML edit', () => {
    const actions = makeActions();
    const setPipelineYaml = vi.fn();
    const { result } = renderHook(() => useMoqYamlSync(actions, setPipelineYaml));

    act(() => result.current.handleYamlChange(moqYaml('out-1')));

    expect(setPipelineYaml).toHaveBeenCalledWith(moqYaml('out-1'));
    expect(actions.setOutputBroadcast).not.toHaveBeenCalled();

    act(() => vi.advanceTimersByTime(300));
    expect(actions.setOutputBroadcast).toHaveBeenLastCalledWith('out-1');
  });

  it('clears stale broadcasts when a non-MoQ pipeline is pasted', () => {
    const actions = makeActions();
    const { result } = renderHook(() => useMoqYamlSync(actions, vi.fn()));

    act(() => result.current.deriveMoqFromYaml(moqYaml('out-1')));
    act(() => result.current.handleYamlChange(nonMoqYaml));
    act(() => vi.advanceTimersByTime(300));

    expect(actions.setOutputBroadcast).toHaveBeenLastCalledWith('');
  });

  it('does not re-derive when only non-client parts of the YAML change', () => {
    const actions = makeActions();
    const { result } = renderHook(() => useMoqYamlSync(actions, vi.fn()));

    act(() => result.current.deriveMoqFromYaml(moqYaml('out-1')));
    const callsAfterDerive = (actions.setOutputBroadcast as ReturnType<typeof vi.fn>).mock.calls
      .length;

    act(() =>
      result.current.handleYamlChange(
        moqYaml('out-1') + '\nnodes:\n  sink:\n    kind: core::sink\n'
      )
    );
    act(() => vi.advanceTimersByTime(300));

    expect(actions.setOutputBroadcast).toHaveBeenCalledTimes(callsAfterDerive);
  });

  it('a synchronous derive cancels a pending edit debounce (no stale overwrite)', () => {
    const actions = makeActions();
    const { result } = renderHook(() => useMoqYamlSync(actions, vi.fn()));

    act(() => result.current.handleYamlChange(moqYaml('edited-out', '/moq/edited')));
    act(() => result.current.deriveMoqFromYaml(moqYaml('tmpl-out', '/moq/tmpl')));
    act(() => vi.advanceTimersByTime(300));

    expect(actions.setOutputBroadcast).toHaveBeenLastCalledWith('tmpl-out');
    expect(actions.setOutputBroadcast).not.toHaveBeenCalledWith('edited-out');
  });

  it('flushPendingDerive applies an in-flight debounced edit immediately', () => {
    const actions = makeActions();
    const { result } = renderHook(() => useMoqYamlSync(actions, vi.fn()));

    act(() => result.current.handleYamlChange(moqYaml('out-1')));
    expect(actions.setOutputBroadcast).not.toHaveBeenCalled();

    act(() => result.current.flushPendingDerive());
    expect(actions.setOutputBroadcast).toHaveBeenLastCalledWith('out-1');

    // The flushed timer must not fire again afterwards.
    act(() => vi.advanceTimersByTime(300));
    expect(actions.setOutputBroadcast).toHaveBeenCalledTimes(1);
  });

  it('flushPendingDerive is a no-op when no edit is pending (preserves manual edits)', () => {
    const actions = makeActions();
    const { result } = renderHook(() => useMoqYamlSync(actions, vi.fn()));

    act(() => result.current.deriveMoqFromYaml(moqYaml('out-1')));
    const callsAfterDerive = (actions.setOutputBroadcast as ReturnType<typeof vi.fn>).mock.calls
      .length;

    act(() => result.current.flushPendingDerive());

    expect(actions.setOutputBroadcast).toHaveBeenCalledTimes(callsAfterDerive);
  });
});
