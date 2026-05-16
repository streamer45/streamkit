// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { describe, expect, it, vi } from 'vitest';

import { utilsLogger } from '@/utils/logger';

import { injectFileReadNode } from './yamlFileReadInjection';

describe('injectFileReadNode — steps form', () => {
  it('replaces a single - kind: streamkit::http_input with core::file_reader using the matched indent', () => {
    const yaml = `mode: oneshot
steps:
  - kind: streamkit::http_input
  - kind: core::dummy_consumer
`;

    const result = injectFileReadNode(yaml, '/tmp/asset.bin');

    expect(typeof result).toBe('string');
    expect(result.includes('core::file_reader')).toBe(true);
    expect(result.includes('streamkit::http_input')).toBe(false);
    expect(result).toContain('  - kind: core::file_reader\n');
    expect(result).toContain('    params:\n');
    expect(result).toContain('      path: "/tmp/asset.bin"\n');
    expect(result).toContain('      chunk_size: 8192');
    expect(result).toContain('- kind: core::dummy_consumer');
    expect(result).toContain('mode: oneshot');
  });

  it('honors a deeper original list-item indentation when emitting the replacement lines', () => {
    const yaml = `mode: oneshot
steps:
    - kind: streamkit::http_input
    - kind: core::dummy_consumer
`;

    const result = injectFileReadNode(yaml, '/data/in.bin');

    expect(result).toContain('    - kind: core::file_reader\n');
    expect(result).toContain('      params:\n');
    expect(result).toContain('        path: "/data/in.bin"\n');
    expect(result).toContain('        chunk_size: 8192');
  });

  it('preserves trailing steps verbatim (steps form does not consume the original params block)', () => {
    const yaml = `mode: oneshot
steps:
  - kind: streamkit::http_input
    params:
      url: http://example.com/data.bin
  - kind: core::dummy_consumer
`;

    const result = injectFileReadNode(yaml, '/tmp/asset.bin');

    expect(result.includes('streamkit::http_input')).toBe(false);
    expect(result).toContain('- kind: core::file_reader');
    expect(result).toContain('url: http://example.com/data.bin');
    expect(result).toContain('- kind: core::dummy_consumer');
  });

  it('replaces every streamkit::http_input occurrence in steps form (no early break inside the helper)', () => {
    const yaml = `mode: oneshot
steps:
  - kind: streamkit::http_input
  - kind: core::middle
  - kind: streamkit::http_input
  - kind: core::dummy_consumer
`;

    const result = injectFileReadNode(yaml, '/tmp/asset.bin');

    expect(result.includes('streamkit::http_input')).toBe(false);
    const fileReaderCount = result.split('- kind: core::file_reader').length - 1;
    expect(fileReaderCount).toBe(2);
    expect(result).toContain('- kind: core::middle');
    expect(result).toContain('- kind: core::dummy_consumer');
  });
});

describe('injectFileReadNode — nodes form', () => {
  it('replaces kind: streamkit::http_input with core::file_reader and drops the literal params: line', () => {
    const yaml = `mode: oneshot
nodes:
  my_input:
    kind: streamkit::http_input
    params:
      timeout: 30
  next_node:
    kind: core::dummy_consumer
`;

    const result = injectFileReadNode(yaml, '/tmp/asset.bin');

    expect(result.includes('streamkit::http_input')).toBe(false);
    expect(result).toContain('    kind: core::file_reader\n');
    expect(result).toContain('    params:\n');
    expect(result).toContain('      path: "/tmp/asset.bin"\n');
    expect(result).toContain('      chunk_size: 8192');

    const originalParamsBlock = `    kind: core::file_reader
    params:
      timeout: 30`;
    expect(result.includes(originalParamsBlock)).toBe(false);
  });

  it('leaves the next node in the nodes: map untouched after replacement', () => {
    const yaml = `mode: oneshot
nodes:
  my_input:
    kind: streamkit::http_input
    params:
      timeout: 30
  next_node:
    kind: core::dummy_consumer
    params:
      gain: 2
`;

    const result = injectFileReadNode(yaml, '/tmp/asset.bin');

    expect(result).toContain('  next_node:');
    expect(result).toContain('    kind: core::dummy_consumer');
    expect(result).toContain('      gain: 2');
  });

  it('drops a fully-indented (4+ space) list-item block belonging to the replaced node', () => {
    const yaml = `mode: oneshot
nodes:
  my_input:
    kind: streamkit::http_input
    params:
        - one
        - two
        - three
  next_node:
    kind: core::dummy_consumer
`;

    const result = injectFileReadNode(yaml, '/tmp/asset.bin');

    expect(result.includes('streamkit::http_input')).toBe(false);
    expect(result.includes('- one')).toBe(false);
    expect(result.includes('- two')).toBe(false);
    expect(result.includes('- three')).toBe(false);
    expect(result).toContain('  next_node:');
    expect(result).toContain('    kind: core::dummy_consumer');
  });
});

describe('injectFileReadNode — edge cases', () => {
  it('returns the original YAML unchanged when no streamkit::http_input is present and logs a warning', () => {
    const warnSpy = vi.spyOn(utilsLogger, 'warn').mockImplementation(() => {});
    const yaml = `mode: oneshot
nodes:
  some_node:
    kind: core::foo
    params:
      bar: 1
`;

    const result = injectFileReadNode(yaml, '/tmp/asset.bin');

    expect(result).toBe(yaml);
    expect(warnSpy).toHaveBeenCalledWith(expect.stringContaining('No streamkit::http_input'));
    warnSpy.mockRestore();
  });

  it('ignores streamkit::http_input that lives outside both steps: and nodes: sections', () => {
    const yaml = `mode: oneshot
metadata:
  kind: streamkit::http_input
`;

    const result = injectFileReadNode(yaml, '/tmp/asset.bin');

    expect(result).toBe(yaml);
    expect(result.includes('core::file_reader')).toBe(false);
    expect(result.includes('streamkit::http_input')).toBe(true);
  });

  it('treats a top-level non-indented line as switching off the active section', () => {
    const yaml = `mode: oneshot
steps:
  - kind: core::pre_marker
other:
  kind: streamkit::http_input
`;

    const result = injectFileReadNode(yaml, '/tmp/asset.bin');

    expect(result).toBe(yaml);
    expect(result.includes('core::file_reader')).toBe(false);
    expect(result).toContain('- kind: core::pre_marker');
    expect(result).toContain('other:');
  });

  it('does not throw on malformed/garbage YAML and returns a string', () => {
    const garbage = ':::: not\n  - valid\n\t\t{}}}}\n%%% --- !!! ${unclosed';

    expect(() => injectFileReadNode(garbage, '/tmp/asset.bin')).not.toThrow();
    const result = injectFileReadNode(garbage, '/tmp/asset.bin');
    expect(typeof result).toBe('string');
    expect(result).toBe(garbage);
  });

  it('preserves a double-quote in assetPath verbatim (current behavior — see follow-up note)', () => {
    const yaml = `mode: oneshot
steps:
  - kind: streamkit::http_input
`;

    const result = injectFileReadNode(yaml, '/tmp/he"llo.bin');

    expect(result).toContain('path: "/tmp/he"llo.bin"');
  });

  it('produces a string for every supported success path', () => {
    const stepsYaml = `steps:
  - kind: streamkit::http_input
`;
    const nodesYaml = `nodes:
  src:
    kind: streamkit::http_input
`;

    const stepsResult = injectFileReadNode(stepsYaml, '/a');
    const nodesResult = injectFileReadNode(nodesYaml, '/b');

    expect(typeof stepsResult).toBe('string');
    expect(typeof nodesResult).toBe('string');
    expect(stepsResult.includes('core::file_reader')).toBe(true);
    expect(nodesResult.includes('core::file_reader')).toBe(true);
    expect(stepsResult.includes('streamkit::http_input')).toBe(false);
    expect(nodesResult.includes('streamkit::http_input')).toBe(false);
  });
});
