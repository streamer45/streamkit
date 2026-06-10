// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { render, screen, act } from '@testing-library/react';
import { useState } from 'react';
import { beforeEach, describe, expect, it } from 'vitest';

let childRenders = 0;

beforeEach(() => {
  childRenders = 0;
});

function Child({ label }: { label: string }) {
  childRenders++;
  return <span>{label}</span>;
}

function Parent() {
  const [count, setCount] = useState(0);
  return (
    <div>
      <button onClick={() => setCount(count + 1)}>{count}</button>
      <Child label="stable" />
    </div>
  );
}

describe('react compiler in vitest pipeline', () => {
  it('compiles components with the memo cache from react/compiler-runtime', () => {
    // Child is left uncompiled on purpose: incrementing the module-level
    // render counter makes the compiler bail out, which is what lets it
    // observe re-renders below.
    expect(String(Parent)).toMatch(/\.c\)\(\d+\)|_c\(\d+\)/);
  });

  it('memoizes children with stable props across parent re-renders', () => {
    render(<Parent />);
    expect(childRenders).toBe(1);
    for (let i = 0; i < 5; i++) {
      act(() => {
        screen.getByRole('button').click();
      });
    }
    expect(screen.getByRole('button').textContent).toBe('5');
    expect(childRenders).toBe(1);
  });
});
