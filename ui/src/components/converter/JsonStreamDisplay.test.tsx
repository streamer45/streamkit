// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { render, screen, waitFor } from '@testing-library/react';
import { describe, it, expect, vi } from 'vitest';

import { componentsLogger } from '@/utils/logger';

import { JsonStreamDisplay } from './JsonStreamDisplay';

vi.mock('@/utils/logger', () => ({
  componentsLogger: {
    debug: vi.fn(),
    info: vi.fn(),
    warn: vi.fn(),
    error: vi.fn(),
  },
}));

function makeStream(chunks: string[]): ReadableStream<Uint8Array> {
  const encoder = new TextEncoder();
  return new ReadableStream({
    start(controller) {
      for (const chunk of chunks) {
        controller.enqueue(encoder.encode(chunk));
      }
      controller.close();
    },
  });
}

describe('JsonStreamDisplay', () => {
  it('shows loading state initially', () => {
    const stream = makeStream([]);
    render(<JsonStreamDisplay stream={stream} />);
    expect(screen.getByText('Processing and streaming JSON output...')).toBeInTheDocument();
  });

  it('renders items from a JSON stream', async () => {
    const stream = makeStream(['{"Transcription": {"text": "hello world"}}\n']);
    render(<JsonStreamDisplay stream={stream} />);

    await waitFor(() => {
      expect(screen.getByText('Transcription')).toBeInTheDocument();
    });
  });

  it('renders custom title', async () => {
    const stream = makeStream(['{"Text": "hi"}\n']);
    render(<JsonStreamDisplay stream={stream} title="Custom Title" />);

    await waitFor(() => {
      expect(screen.getByText('Custom Title')).toBeInTheDocument();
    });
  });

  it('renders count badge', async () => {
    const stream = makeStream(['{"a":1}\n{"b":2}\n']);
    render(<JsonStreamDisplay stream={stream} />);

    await waitFor(() => {
      expect(screen.getByText('2')).toBeInTheDocument();
    });
  });

  it('handles parse errors gracefully', async () => {
    const stream = makeStream(['{invalid json}\n']);
    render(<JsonStreamDisplay stream={stream} />);

    await waitFor(() => {
      expect(screen.getByText('ParseError')).toBeInTheDocument();
    });
  });

  it('renders Custom packet type with type_id hint', async () => {
    const stream = makeStream([
      JSON.stringify({ Custom: { type_id: 'my_type', data: { event_type: 'foo' } } }) + '\n',
    ]);
    render(<JsonStreamDisplay stream={stream} />);

    await waitFor(() => {
      expect(screen.getByText('Custom')).toBeInTheDocument();
      expect(screen.getByText('my_type · foo')).toBeInTheDocument();
    });
  });

  it('calls onComplete when stream finishes', async () => {
    const onComplete = vi.fn();
    const stream = makeStream(['{"ok":true}\n']);
    render(<JsonStreamDisplay stream={stream} onComplete={onComplete} />);

    await waitFor(() => {
      expect(onComplete).toHaveBeenCalledTimes(1);
    });
  });

  it('renders Text packet kind', async () => {
    const stream = makeStream(['{"Text": "hello"}\n']);
    render(<JsonStreamDisplay stream={stream} />);

    await waitFor(() => {
      expect(screen.getByText('Text')).toBeInTheDocument();
    });
  });

  it('renders Binary packet kind', async () => {
    const stream = makeStream(['{"Binary": [1,2,3]}\n']);
    render(<JsonStreamDisplay stream={stream} />);

    await waitFor(() => {
      expect(screen.getByText('Binary')).toBeInTheDocument();
    });
  });

  it('renders Audio packet kind', async () => {
    const stream = makeStream(['{"Audio": {"samples": []}}\n']);
    render(<JsonStreamDisplay stream={stream} />);

    await waitFor(() => {
      expect(screen.getByText('Audio')).toBeInTheDocument();
    });
  });

  it('renders single-key object with that key as label', async () => {
    const stream = makeStream(['{"MyCustomKey": 42}\n']);
    render(<JsonStreamDisplay stream={stream} />);

    await waitFor(() => {
      expect(screen.getByText('MyCustomKey')).toBeInTheDocument();
    });
  });

  it('renders multi-key object as "object"', async () => {
    const stream = makeStream(['{"a":1,"b":2}\n']);
    render(<JsonStreamDisplay stream={stream} />);

    await waitFor(() => {
      expect(screen.getByText('object')).toBeInTheDocument();
    });
  });

  it('renders array values with typeof as label', async () => {
    const stream = makeStream(['[1, 2, 3]\n']);
    render(<JsonStreamDisplay stream={stream} />);

    await waitFor(() => {
      expect(screen.getByText('object')).toBeInTheDocument();
    });
  });

  it('handles concatenated JSON objects without newlines', async () => {
    const stream = makeStream(['{"a":1}{"b":2}']);
    render(<JsonStreamDisplay stream={stream} />);

    await waitFor(() => {
      expect(screen.getByText('2')).toBeInTheDocument();
    });
  });

  it('warns and skips if stream is already locked', async () => {
    const stream = makeStream(['{"ok":true}\n']);
    stream.getReader();

    render(<JsonStreamDisplay stream={stream} />);

    await waitFor(() => {
      expect(screen.queryByText('ok')).not.toBeInTheDocument();
      expect(componentsLogger.warn).toHaveBeenCalled();
    });
  });
});
