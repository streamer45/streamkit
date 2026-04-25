// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { render, screen, fireEvent } from '@testing-library/react';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('jotai/react', () => ({
  useAtomValue: () => ({}),
}));

vi.mock('@/stores/sessionAtoms', () => ({
  nodeParamsAtom: () => 'mock-atom',
}));

// Import after mocks are set up
const { default: InspectorPane } = await import('./InspectorPane');

const baseNodeDefinition = {
  kind: 'test-node',
  description: 'A test node',
  inputs: [],
  outputs: [],
  param_schema: {},
};

const baseNode = {
  id: 'node-1',
  type: 'custom',
  position: { x: 0, y: 0 },
  data: { label: 'TestNode', kind: 'test-node', params: {} },
};

describe('InspectorPane', () => {
  let onParamChange: ReturnType<typeof vi.fn>;
  let onLabelChange: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    onParamChange = vi.fn();
    onLabelChange = vi.fn();
  });

  it('renders a select dropdown for enum-constrained string properties', () => {
    const definition = {
      ...baseNodeDefinition,
      param_schema: {
        properties: {
          resolution: {
            type: 'string',
            enum: ['640x480', '1280x720', '1920x1080'],
            description: 'Viewport resolution',
          },
        },
      },
    };

    render(
      <InspectorPane
        node={{ ...baseNode, data: { ...baseNode.data, params: { resolution: '1280x720' } } }}
        nodeDefinition={definition}
        onParamChange={onParamChange}
        onLabelChange={onLabelChange}
      />,
    );

    const select = screen.getByRole('combobox', { name: 'Viewport resolution' });
    expect(select).toBeInTheDocument();
    expect(select).toHaveValue('1280x720');

    const options = screen.getAllByRole('option');
    expect(options).toHaveLength(3);
    expect(options[0]).toHaveTextContent('640x480');
    expect(options[1]).toHaveTextContent('1280x720');
    expect(options[2]).toHaveTextContent('1920x1080');
  });

  it('falls back to first enum value when current value does not match any option', () => {
    const definition = {
      ...baseNodeDefinition,
      param_schema: {
        properties: {
          resolution: {
            type: 'string',
            enum: ['640x480', '1280x720', '1920x1080'],
            description: 'Viewport resolution',
            tunable: true,
          },
        },
      },
    };

    render(
      <InspectorPane
        node={baseNode}
        nodeDefinition={definition}
        onParamChange={onParamChange}
        onLabelChange={onLabelChange}
      />,
    );

    const select = screen.getByRole('combobox', { name: 'Viewport resolution' });
    expect(select).toHaveValue('640x480');
  });

  it('sends UpdateParams on selection change', () => {
    const definition = {
      ...baseNodeDefinition,
      param_schema: {
        properties: {
          mode: {
            type: 'string',
            enum: ['fast', 'balanced', 'quality'],
            description: 'Processing mode',
            tunable: true,
          },
        },
      },
    };

    render(
      <InspectorPane
        node={{ ...baseNode, data: { ...baseNode.data, params: { mode: 'fast' } } }}
        nodeDefinition={definition}
        onParamChange={onParamChange}
        onLabelChange={onLabelChange}
        isMonitorView
      />,
    );

    const select = screen.getByRole('combobox', { name: 'Processing mode' });
    fireEvent.change(select, { target: { value: 'quality' } });

    expect(onParamChange).toHaveBeenCalledWith('node-1', 'mode', 'quality');
  });

  it('renders a text input for non-enum string properties', () => {
    const definition = {
      ...baseNodeDefinition,
      param_schema: {
        properties: {
          title: {
            type: 'string',
            description: 'Display title',
          },
        },
      },
    };

    render(
      <InspectorPane
        node={baseNode}
        nodeDefinition={definition}
        onParamChange={onParamChange}
        onLabelChange={onLabelChange}
      />,
    );

    expect(screen.queryByRole('combobox')).not.toBeInTheDocument();
    expect(screen.getByRole('textbox', { name: 'Display title' })).toBeInTheDocument();
  });

  it('disables enum select for non-tunable params in monitor view', () => {
    const definition = {
      ...baseNodeDefinition,
      param_schema: {
        properties: {
          resolution: {
            type: 'string',
            enum: ['640x480', '1280x720'],
            description: 'Viewport resolution',
            tunable: false,
          },
        },
      },
    };

    render(
      <InspectorPane
        node={{ ...baseNode, data: { ...baseNode.data, params: { resolution: '640x480' } } }}
        nodeDefinition={definition}
        onParamChange={onParamChange}
        onLabelChange={onLabelChange}
        isMonitorView
      />,
    );

    const select = screen.getByRole('combobox', { name: 'Viewport resolution' });
    expect(select).toBeDisabled();
  });
});
