// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { render, screen, act } from '@testing-library/react';
import { beforeEach, describe, expect, it } from 'vitest';

import { getChildRenders, Parent, resetChildRenders } from './reactCompilerFixture';

describe('react compiler in vitest pipeline', () => {
  beforeEach(() => {
    resetChildRenders();
  });

  // Child is not wrapped in React.memo, so it can only render once across
  // parent re-renders if the compiler memoized its element in Parent. This
  // fails if the compiler is not active in the test pipeline.
  it('memoizes children with stable props across parent re-renders', () => {
    render(<Parent />);
    expect(getChildRenders()).toBe(1);
    for (let i = 0; i < 5; i++) {
      act(() => {
        screen.getByRole('button').click();
      });
    }
    expect(screen.getByRole('button').textContent).toBe('5');
    expect(getChildRenders()).toBe(1);
  });
});
