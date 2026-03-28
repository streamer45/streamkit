// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { describe, it, expect } from 'vitest';

import { fontFamilyForAsset } from './fontAssets';

describe('fontFamilyForAsset', () => {
  it('derives sk- prefixed family from system font path', () => {
    expect(fontFamilyForAsset('samples/fonts/system/Inter.ttf')).toBe('sk-Inter');
  });

  it('strips file extension', () => {
    expect(fontFamilyForAsset('samples/fonts/system/DejaVuSans.ttf')).toBe('sk-DejaVuSans');
    expect(fontFamilyForAsset('samples/fonts/user/CustomFont.otf')).toBe('sk-CustomFont');
  });

  it('handles bold variants', () => {
    expect(fontFamilyForAsset('samples/fonts/system/DejaVuSans-Bold.ttf')).toBe(
      'sk-DejaVuSans-Bold'
    );
  });

  it('handles bare filename without path', () => {
    expect(fontFamilyForAsset('Roboto.ttf')).toBe('sk-Roboto');
  });

  it('handles path with no extension', () => {
    expect(fontFamilyForAsset('samples/fonts/system/NoExt')).toBe('sk-NoExt');
  });
});
