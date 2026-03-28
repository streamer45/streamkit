// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import fs from 'node:fs';
import path from 'node:path';

import { describe, it, expect } from 'vitest';

import { cssFontFamily, isBoldFont } from './compositorCanvasLayers';

describe('cssFontFamily', () => {
  it('returns custom sk- family as primary with DejaVu fallback', () => {
    const result = cssFontFamily('samples/fonts/system/DejaVuSans.ttf');
    expect(result).toBe('"sk-DejaVuSans", "DejaVu Sans", "Verdana", sans-serif');
  });

  it('returns serif fallback for DejaVuSerif', () => {
    const result = cssFontFamily('samples/fonts/system/DejaVuSerif.ttf');
    expect(result).toContain('serif');
    expect(result).toContain('"sk-DejaVuSerif"');
  });

  it('falls back to generic sans-serif for unknown fonts', () => {
    const result = cssFontFamily('samples/fonts/system/PlayfairDisplay.ttf');
    expect(result).toBe('"sk-PlayfairDisplay", sans-serif');
  });

  it('always includes the sk- custom family as the first entry', () => {
    const fonts = [
      'samples/fonts/system/Inter.ttf',
      'samples/fonts/system/FiraCode.ttf',
      'samples/fonts/user/MyFont.otf',
    ];
    for (const font of fonts) {
      const result = cssFontFamily(font);
      expect(result).toMatch(/^"sk-[^"]+"/);
    }
  });
});

describe('isBoldFont', () => {
  it('detects bold font names', () => {
    expect(isBoldFont('samples/fonts/system/DejaVuSans-Bold.ttf')).toBe(true);
    expect(isBoldFont('samples/fonts/system/DejaVuSerif-Bold.ttf')).toBe(true);
  });

  it('returns false for regular font names', () => {
    expect(isBoldFont('samples/fonts/system/DejaVuSans.ttf')).toBe(false);
    expect(isBoldFont('samples/fonts/system/Inter.ttf')).toBe(false);
  });
});

describe('index.css font override guard', () => {
  it('must not use !important on a wildcard descendant selector inside .react-flow', () => {
    // This test prevents a regression where `.react-flow *` with `!important`
    // overrides inline font-family on compositor canvas text overlays,
    // making all text render in the UI font regardless of the selected asset font.
    const cssPath = path.resolve(__dirname, '../index.css');
    const css = fs.readFileSync(cssPath, 'utf-8');

    // Match patterns like `.react-flow *` or `.react-flow, .react-flow *`
    // followed by a block containing `font-family: ... !important`.
    const dangerousPattern =
      /\.react-flow\s*(?:,\s*\.react-flow)?\s+\*\s*\{[^}]*font-family:[^}]*!important/;
    expect(css).not.toMatch(dangerousPattern);
  });
});
