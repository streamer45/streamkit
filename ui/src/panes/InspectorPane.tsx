// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import type { Node } from '@xyflow/react';
import { useAtomValue } from 'jotai/react';
import React from 'react';

import { SKTooltip } from '@/components/Tooltip';
import { CheckboxWithLabel } from '@/components/ui/Checkbox';
import { nodeParamsAtom } from '@/stores/sessionAtoms';
import type { NodeDefinition, InputPin, OutputPin, PacketType } from '@/types/types';
import { readByPath } from '@/utils/controlProps';
import type { JsonSchema, JsonSchemaProperty } from '@/utils/jsonSchema';
import {
  formatPacketType,
  getPacketTypeColor,
  formatPinCardinality,
  getPinCardinalityIcon,
  getPinCardinalityDescription,
} from '@/utils/packetTypes';

const PaneWrapper = styled.div`
  display: flex;
  flex-direction: column;
  height: 100%;
  overflow: hidden;
`;

const PaneHeader = styled.div`
  padding: 12px;
  border-bottom: 1px solid var(--sk-border);
  flex-shrink: 0;
`;

const PaneTitle = styled.h3`
  margin: 0 0 4px 0;
  font-size: 14px;
  font-weight: 600;
  color: var(--sk-text);
`;

const PaneSubtitle = styled.p`
  margin: 0;
  font-size: 12px;
  color: var(--sk-text-muted);
`;

const ContentWrapper = styled.div`
  flex: 1;
  overflow-y: auto;
  padding: 12px;
  display: flex;
  flex-direction: column;
  gap: 12px;
`;

const FormInput = styled.input`
  width: 100%;
  max-width: 100%;
  padding: 8px;
  background: var(--sk-panel-bg);
  border: 1px solid var(--sk-border);
  border-radius: 4px;
  color: var(--sk-text);
  box-sizing: border-box;

  &:focus-visible {
    outline: 1px solid var(--sk-primary);
  }
`;

const FormSelect = styled.select`
  width: 100%;
  max-width: 100%;
  padding: 8px;
  background: var(--sk-panel-bg);
  border: 1px solid var(--sk-border);
  border-radius: 4px;
  color: var(--sk-text);
  box-sizing: border-box;
  font-family: inherit;
  cursor: pointer;

  &:focus-visible {
    outline: 1px solid var(--sk-primary);
  }

  &:disabled {
    cursor: not-allowed;
    opacity: 0.6;
  }
`;

const FormTextarea = styled.textarea`
  width: 100%;
  max-width: 100%;
  padding: 8px;
  background: var(--sk-panel-bg);
  border: 1px solid var(--sk-border);
  border-radius: 4px;
  color: var(--sk-text);
  box-sizing: border-box;
  font-family: var(--sk-font-code);
  font-size: 12px;
  line-height: 1.4;
  resize: vertical;
  min-height: 140px;
  white-space: pre;

  &:focus-visible {
    outline: 1px solid var(--sk-primary);
  }
`;

const FormSection = styled.div`
  background: var(--sk-bg);
  border: 1px solid var(--sk-border);
  border-radius: 8px;
  padding: 16px;
  display: flex;
  flex-direction: column;
  gap: 16px;
`;

const PinList = styled.ul`
  list-style: none;
  padding: 0;
  margin: 0;
  display: flex;
  flex-direction: column;
  gap: 6px;
`;

const PinItem = styled.li`
  display: flex;
  align-items: center;
  gap: 8px;
`;

const ColorDot = styled.span<{ color: string }>`
  width: 10px;
  height: 10px;
  border-radius: 4px;
  background: ${(p) => p.color};
  border: 1px solid var(--sk-border-strong);
  display: inline-block;
`;

interface InspectorPaneProps {
  node: Node<{ label: string; kind: string; params: Record<string, unknown>; sessionId?: string }>;
  nodeDefinition: NodeDefinition;
  onParamChange: (nodeId: string, paramName: string, value: unknown) => void;
  onLabelChange: (nodeId: string, newLabel: string) => void;
  readOnly?: boolean;
  /** When true (Monitor view), non-tunable params are disabled */
  isMonitorView?: boolean;
}

// Helper: Render string field
const StringField: React.FC<{
  inputId: string;
  value: unknown;
  schema: JsonSchemaProperty;
  paramKey: string;
  readOnly: boolean;
  onChange: (value: string) => void;
}> = ({ inputId, value, schema, paramKey, readOnly, onChange }) => {
  const stringValue = String(value);
  const keyLower = paramKey.toLowerCase();
  const descLower = (schema.description ?? '').toLowerCase();

  const shouldUseTextarea =
    keyLower === 'script' ||
    keyLower.endsWith('_script') ||
    descLower.includes('javascript') ||
    descLower.includes('js code') ||
    stringValue.includes('\n');

  if (shouldUseTextarea) {
    return (
      <FormTextarea
        id={inputId}
        value={stringValue}
        onChange={(e) => onChange(e.target.value)}
        placeholder={schema.description}
        disabled={readOnly}
        aria-label={schema.description}
        spellCheck={false}
      />
    );
  }

  return (
    <FormInput
      id={inputId}
      type="text"
      value={stringValue}
      onChange={(e) => onChange(e.target.value)}
      placeholder={schema.description}
      disabled={readOnly}
      aria-label={schema.description}
    />
  );
};

// Helper: Render select field for enum-constrained strings
const SelectField: React.FC<{
  inputId: string;
  value: unknown;
  schema: JsonSchemaProperty;
  readOnly: boolean;
  onChange: (value: string) => void;
}> = ({ inputId, value, schema, readOnly, onChange }) => {
  const options = (schema.enum ?? []).map(String);
  const defaultStr = schema.default != null ? String(schema.default) : undefined;
  const fallback = (defaultStr && options.includes(defaultStr) ? defaultStr : options[0]) ?? '';
  const resolved = options.includes(String(value)) ? String(value) : fallback;
  return (
    <FormSelect
      id={inputId}
      value={resolved}
      onChange={(e) => onChange(e.target.value)}
      disabled={readOnly}
      aria-label={schema.description}
    >
      {options.map((opt) => (
        <option key={opt} value={opt}>
          {opt}
        </option>
      ))}
    </FormSelect>
  );
};

// Helper: Render number field
const NumberField: React.FC<{
  inputId: string;
  value: unknown;
  schema: JsonSchemaProperty;
  readOnly: boolean;
  onChange: (value: number) => void;
}> = ({ inputId, value, schema, readOnly, onChange }) => (
  <FormInput
    id={inputId}
    type="number"
    value={Number(value)}
    onChange={(e) => onChange(parseFloat(e.target.value))}
    placeholder={schema.description}
    disabled={readOnly}
    aria-label={schema.description}
    min={schema.minimum}
    max={schema.maximum}
    step={schema.multipleOf || schema.type === 'integer' ? 1 : 'any'}
  />
);

// Helper: Render boolean field
const BooleanField: React.FC<{
  inputId: string;
  value: unknown;
  schema: JsonSchemaProperty;
  readOnly: boolean;
  onChange: (value: boolean) => void;
}> = ({ inputId, value, schema, readOnly, onChange }) => (
  <CheckboxWithLabel
    id={inputId}
    checked={!!value}
    onCheckedChange={(checked) => onChange(checked)}
    disabled={readOnly}
    label={schema.description}
  />
);

// Helper: Render JSON field (fallback for unknown types)
const JsonField: React.FC<{
  inputId: string;
  value: unknown;
  schema: JsonSchemaProperty;
  readOnly: boolean;
  onChange: (value: unknown) => void;
}> = ({ inputId, value, schema, readOnly, onChange }) => (
  <FormInput
    id={inputId}
    type="text"
    value={JSON.stringify(value)}
    onChange={(e) => {
      try {
        onChange(JSON.parse(e.target.value));
      } catch {
        onChange(e.target.value);
      }
    }}
    placeholder={schema.description}
    disabled={readOnly}
    aria-label={schema.description}
  />
);

const InspectorPane: React.FC<InspectorPaneProps> = ({
  node,
  nodeDefinition,
  onParamChange,
  onLabelChange,
  readOnly = false,
  isMonitorView = false,
}) => {
  // Read live params for this node from a per-node Jotai atom (fine-grained reactivity).
  // Only re-renders when THIS node's params change, not when any node's params change.
  const paramsKey = node.data.sessionId ? `${node.data.sessionId}\0${node.id}` : node.id;
  const nodeParams = useAtomValue(nodeParamsAtom(paramsKey));

  const handleInputChange = (key: string, value: unknown, schema?: JsonSchemaProperty) => {
    // When a schema has a `path` override, build the nested payload and
    // route it through onParamChange so it works in both Monitor view
    // (where onParamChange → tuneNode) and design view (where
    // onParamChange → onConfigChange).  The path is passed as the
    // paramName so the caller can build the correct UpdateParams.
    if (schema?.path) {
      onParamChange(node.id, schema.path, value);
    } else {
      onParamChange(node.id, key, value);
    }
  };

  const renderField = (key: string, schema: JsonSchemaProperty) => {
    const readPath = schema.path ?? key;
    const currentValue =
      readByPath(nodeParams as Record<string, unknown>, readPath) ??
      readByPath((node.data.params ?? {}) as Record<string, unknown>, readPath) ??
      schema.default ??
      '';
    const inputId = `param-${node.id}-${key}`;
    // In monitor view, disable non-tunable params (they can't be changed at runtime)
    const isDisabled = readOnly || (isMonitorView && !schema.tunable);

    switch (schema.type) {
      case 'string':
        if (schema.enum && schema.enum.length > 0) {
          return (
            <SelectField
              inputId={inputId}
              value={currentValue}
              schema={schema}
              readOnly={isDisabled}
              onChange={(v) => handleInputChange(key, v, schema)}
            />
          );
        }
        return (
          <StringField
            inputId={inputId}
            value={currentValue}
            schema={schema}
            paramKey={key}
            readOnly={isDisabled}
            onChange={(v) => handleInputChange(key, v, schema)}
          />
        );
      case 'number':
      case 'integer':
        return (
          <NumberField
            inputId={inputId}
            value={currentValue}
            schema={schema}
            readOnly={isDisabled}
            onChange={(v) => handleInputChange(key, v, schema)}
          />
        );
      case 'boolean':
        return (
          <BooleanField
            inputId={inputId}
            value={currentValue}
            schema={schema}
            readOnly={isDisabled}
            onChange={(v) => handleInputChange(key, v, schema)}
          />
        );
      default:
        return (
          <JsonField
            inputId={inputId}
            value={currentValue}
            schema={schema}
            readOnly={isDisabled}
            onChange={(v) => handleInputChange(key, v, schema)}
          />
        );
    }
  };

  const properties = ((nodeDefinition.param_schema as JsonSchema | undefined)?.properties ??
    {}) as Record<string, JsonSchemaProperty>;

  return (
    <PaneWrapper>
      <PaneHeader>
        <PaneTitle>Inspector</PaneTitle>
        <PaneSubtitle className="code-font">{node.data.label}</PaneSubtitle>
        {nodeDefinition.description && (
          <PaneSubtitle style={{ marginTop: 4 }}>{nodeDefinition.description}</PaneSubtitle>
        )}
      </PaneHeader>
      <ContentWrapper>
        <FormSection>
          <div style={{ display: 'flex', flexDirection: 'column', gap: '4px' }}>
            <label
              htmlFor={`node-label-${node.id}`}
              style={{ display: 'block', fontWeight: 'bold', fontSize: '14px' }}
            >
              <span className="code-font">name</span>
              <span style={{ marginLeft: 8, fontSize: 12, color: 'var(--sk-text-muted)' }}>
                Rename node
              </span>
            </label>
            <FormInput
              id={`node-label-${node.id}`}
              type="text"
              value={node.data.label}
              onChange={(e) => onLabelChange(node.id, e.target.value)}
              placeholder="Enter node name"
              disabled={readOnly}
              aria-label="Node name"
            />
          </div>
        </FormSection>

        <FormSection>
          <div style={{ display: 'flex', flexDirection: 'column', gap: 12 }}>
            <div style={{ display: 'flex', gap: 24 }}>
              <div style={{ flex: 1, minWidth: 0 }}>
                <div style={{ fontWeight: 'bold', marginBottom: 6 }}>Inputs</div>
                {Array.isArray(nodeDefinition.inputs) && nodeDefinition.inputs.length > 0 ? (
                  <PinList>
                    {nodeDefinition.inputs.map((inp: InputPin) => {
                      const primaryType = (inp.accepts_types?.[0] ??
                        'Any') as unknown as PacketType;
                      const color = getPacketTypeColor(primaryType);
                      const cardinalityIcon = getPinCardinalityIcon(inp.cardinality);
                      const cardinalityText = formatPinCardinality(inp.cardinality);
                      const cardinalityDescription = getPinCardinalityDescription(
                        inp.cardinality,
                        true
                      );
                      return (
                        <PinItem key={inp.name}>
                          <ColorDot color={color} />
                          <div style={{ flex: 1 }}>
                            <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
                              <span className="code-font" style={{ fontWeight: 600 }}>
                                {inp.name}
                              </span>
                              <SKTooltip
                                content={
                                  <div style={{ fontSize: 11 }}>
                                    <div style={{ fontWeight: 600, marginBottom: 4 }}>
                                      {cardinalityText}
                                    </div>
                                    <div>{cardinalityDescription}</div>
                                  </div>
                                }
                              >
                                <span
                                  style={{
                                    fontSize: 11,
                                    color: 'var(--sk-text-muted)',
                                    cursor: 'help',
                                  }}
                                >
                                  {cardinalityIcon}
                                </span>
                              </SKTooltip>
                            </div>
                            <div style={{ fontSize: 12, color: 'var(--sk-text-muted)' }}>
                              {(inp.accepts_types || [])
                                .map((t) => formatPacketType(t as unknown as PacketType))
                                .join(' | ')}
                            </div>
                          </div>
                        </PinItem>
                      );
                    })}
                  </PinList>
                ) : (
                  <div style={{ fontSize: 12, color: 'var(--sk-text-muted)' }}>No inputs</div>
                )}
              </div>

              <div style={{ flex: 1, minWidth: 0 }}>
                <div style={{ fontWeight: 'bold', marginBottom: 6 }}>Outputs</div>
                {Array.isArray(nodeDefinition.outputs) && nodeDefinition.outputs.length > 0 ? (
                  <PinList>
                    {nodeDefinition.outputs.map((outp: OutputPin) => {
                      const color = getPacketTypeColor(outp.produces_type as unknown as PacketType);
                      const cardinalityIcon = getPinCardinalityIcon(outp.cardinality);
                      const cardinalityText = formatPinCardinality(outp.cardinality);
                      const cardinalityDescription = getPinCardinalityDescription(
                        outp.cardinality,
                        false
                      );
                      return (
                        <PinItem key={outp.name}>
                          <ColorDot color={color} />
                          <div style={{ flex: 1 }}>
                            <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
                              <span className="code-font" style={{ fontWeight: 600 }}>
                                {outp.name}
                              </span>
                              <SKTooltip
                                content={
                                  <div style={{ fontSize: 11 }}>
                                    <div style={{ fontWeight: 600, marginBottom: 4 }}>
                                      {cardinalityText}
                                    </div>
                                    <div>{cardinalityDescription}</div>
                                  </div>
                                }
                              >
                                <span
                                  style={{
                                    fontSize: 11,
                                    color: 'var(--sk-text-muted)',
                                    cursor: 'help',
                                  }}
                                >
                                  {cardinalityIcon}
                                </span>
                              </SKTooltip>
                            </div>
                            <div style={{ fontSize: 12, color: 'var(--sk-text-muted)' }}>
                              {formatPacketType(outp.produces_type as unknown as PacketType)}
                            </div>
                          </div>
                        </PinItem>
                      );
                    })}
                  </PinList>
                ) : (
                  <div style={{ fontSize: 12, color: 'var(--sk-text-muted)' }}>No outputs</div>
                )}
              </div>
            </div>
          </div>
        </FormSection>

        {Object.keys(properties).length === 0 ? (
          <p style={{ fontSize: '12px', color: 'var(--sk-text-muted)' }}>
            This node has no configurable parameters.
          </p>
        ) : (
          <FormSection>
            {Object.entries(properties).map(([key, schema]: [string, JsonSchemaProperty]) => {
              const inputId = `param-${node.id}-${key}`;
              return (
                <div
                  key={key}
                  style={{ width: '100%', display: 'flex', flexDirection: 'column', gap: '4px' }}
                >
                  <label
                    htmlFor={inputId}
                    style={{ display: 'block', fontWeight: 'bold', fontSize: '14px' }}
                  >
                    <span className="code-font">{key}</span>
                    {schema.description && (
                      <span
                        style={{
                          fontWeight: 'normal',
                          fontSize: '0.9em',
                          color: 'var(--sk-text-muted)',
                        }}
                      >
                        {' - ' + schema.description}
                      </span>
                    )}
                  </label>
                  {renderField(key, schema)}
                </div>
              );
            })}
          </FormSection>
        )}
      </ContentWrapper>
    </PaneWrapper>
  );
};

export default InspectorPane;
