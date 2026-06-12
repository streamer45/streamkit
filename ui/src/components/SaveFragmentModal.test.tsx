// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { render, screen } from '@testing-library/react';
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

import SaveFragmentModal from './SaveFragmentModal';

describe('SaveFragmentModal', () => {
  const defaultProps = {
    isOpen: true,
    onClose: vi.fn(),
    onSave: vi.fn(),
  };

  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('renders fields with initial values when open', () => {
    render(
      <SaveFragmentModal
        {...defaultProps}
        initialName="My Fragment"
        initialDescription="A description"
        initialTags={['audio', 'video']}
      />
    );

    expect(screen.getByLabelText(/Fragment Name/)).toHaveValue('My Fragment');
    expect(screen.getByLabelText('Description')).toHaveValue('A description');
    expect(screen.getByLabelText(/Tags/)).toHaveValue('audio, video');
  });

  it('focuses the name input after the open delay', () => {
    render(<SaveFragmentModal {...defaultProps} />);

    vi.runAllTimers();

    expect(screen.getByLabelText(/Fragment Name/)).toHaveFocus();
  });

  it('still focuses when a re-render happens before the delay elapses', () => {
    const { rerender } = render(<SaveFragmentModal {...defaultProps} initialTags={['a']} />);

    // Fresh array identity, as DesignView produces on every render.
    rerender(<SaveFragmentModal {...defaultProps} initialTags={['a']} />);
    vi.runAllTimers();

    expect(screen.getByLabelText(/Fragment Name/)).toHaveFocus();
  });

  it('clears the pending focus timer on unmount', () => {
    const { unmount } = render(<SaveFragmentModal {...defaultProps} />);

    // Radix schedules an unrelated timer, so assert on the delta rather than
    // expecting zero timers after unmount.
    const before = vi.getTimerCount();
    expect(before).toBeGreaterThan(0);
    unmount();

    expect(vi.getTimerCount()).toBeLessThan(before);
  });
});
