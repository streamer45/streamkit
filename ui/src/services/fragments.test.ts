// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { beforeEach, describe, expect, it, vi } from 'vitest';
import YAML from 'yaml';

import type { SamplePipeline } from '@/types/generated/api-types';

import {
  decodeFragmentMetadata,
  deleteFragment,
  fragmentToYaml,
  samplesToFragments,
  saveFragment,
  yamlToFragment,
} from './fragments';
import * as samples from './samples';

vi.mock('./samples', () => ({
  saveSample: vi.fn(),
  deleteSample: vi.fn(),
}));

const FRAGMENT_SAMPLE: SamplePipeline = {
  id: 'frag-1',
  name: 'Audio In',
  description: 'tags:audio,demo|A useful fragment',
  yaml: 'nodes: {}',
  is_system: false,
  mode: 'oneshot',
  is_fragment: true,
  group: null,
  variant: null,
  category: null,
  tags: [],
};

beforeEach(() => {
  vi.clearAllMocks();
});

describe('decodeFragmentMetadata', () => {
  it('returns empty tags and the original description when no "tags:" prefix exists', () => {
    expect(decodeFragmentMetadata('just a description')).toEqual({
      tags: [],
      description: 'just a description',
    });
  });

  it('parses comma-separated tags and the description', () => {
    expect(decodeFragmentMetadata('tags:audio,video|Cool fragment')).toEqual({
      tags: ['audio', 'video'],
      description: 'Cool fragment',
    });
  });

  it('trims whitespace and drops empty tags', () => {
    expect(decodeFragmentMetadata('tags:audio,  ,video, |desc')).toEqual({
      tags: ['audio', 'video'],
      description: 'desc',
    });
  });

  it('preserves "|" characters inside the description body', () => {
    expect(decodeFragmentMetadata('tags:a|left|right')).toEqual({
      tags: ['a'],
      description: 'left|right',
    });
  });

  it('returns empty tags when the prefix matches but no tags follow', () => {
    expect(decodeFragmentMetadata('tags:|only desc')).toEqual({
      tags: [],
      description: 'only desc',
    });
  });
});

describe('samplesToFragments', () => {
  it('keeps only is_fragment=true entries and decodes metadata', () => {
    const nonFragment: SamplePipeline = { ...FRAGMENT_SAMPLE, id: 'plain', is_fragment: false };
    const result = samplesToFragments([FRAGMENT_SAMPLE, nonFragment]);
    expect(result).toHaveLength(1);
    expect(result[0]).toMatchObject({
      id: 'frag-1',
      tags: ['audio', 'demo'],
      description: 'A useful fragment',
    });
  });
});

describe('fragmentToYaml + yamlToFragment', () => {
  it('round-trips a nodes record', () => {
    const nodes = {
      mic: { kind: 'core::mic', params: { rate: 48000 } },
      out: { kind: 'core::output', needs: 'mic' },
    };

    const yaml = fragmentToYaml(nodes);
    expect(typeof yaml).toBe('string');
    expect(YAML.parse(yaml)).toEqual({ nodes });

    expect(yamlToFragment(yaml)).toEqual({ nodes });
  });

  it('returns empty nodes when the YAML has no nodes key', () => {
    const yaml = YAML.stringify({ other: 'value' });
    expect(yamlToFragment(yaml)).toEqual({ nodes: {} });
  });
});

describe('saveFragment', () => {
  it('encodes tags into description, serializes nodes to YAML, and forwards to saveSample', async () => {
    const saved: SamplePipeline = { ...FRAGMENT_SAMPLE };
    vi.mocked(samples.saveSample).mockResolvedValue(saved);

    const result = await saveFragment('Audio In', 'A useful fragment', ['audio', 'demo'], {
      mic: { kind: 'core::mic' },
    });

    expect(result).toBe(saved);
    expect(samples.saveSample).toHaveBeenCalledTimes(1);
    const req = vi.mocked(samples.saveSample).mock.calls[0][0];
    expect(req.name).toBe('Audio In');
    expect(req.description).toBe('tags:audio,demo|A useful fragment');
    expect(req.overwrite).toBe(false);
    expect(req.is_fragment).toBe(true);
    expect(YAML.parse(req.yaml)).toEqual({ nodes: { mic: { kind: 'core::mic' } } });
  });

  it('omits the tags: prefix when tags are empty', async () => {
    vi.mocked(samples.saveSample).mockResolvedValue(FRAGMENT_SAMPLE);

    await saveFragment('n', 'd', [], {});

    expect(vi.mocked(samples.saveSample).mock.calls[0][0].description).toBe('d');
  });
});

describe('deleteFragment', () => {
  it('delegates to deleteSample', async () => {
    vi.mocked(samples.deleteSample).mockResolvedValue();

    await deleteFragment('frag-1');

    expect(samples.deleteSample).toHaveBeenCalledWith('frag-1');
  });
});
