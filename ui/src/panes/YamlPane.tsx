// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { acceptCompletion, completionStatus, startCompletion } from '@codemirror/autocomplete';
import { yaml as yamlLang } from '@codemirror/lang-yaml';
import type { Extension, Range } from '@codemirror/state';
import { Prec, StateEffect, StateEffectType, StateField } from '@codemirror/state';
import { Decoration, EditorView, keymap } from '@codemirror/view';
import styled from '@emotion/styled';
import { solarizedDark, solarizedLight } from '@uiw/codemirror-theme-solarized';
import CodeMirror from '@uiw/react-codemirror';
import { debounce } from 'lodash-es';
import React, { useCallback, useMemo, useRef, useEffect } from 'react';

import { CopyButton } from '@/components/CopyButton';
import { useCompositorSelection } from '@/hooks/useCompositorSelection';
import { useResolvedColorMode } from '@/hooks/useResolvedColorMode';
import type { NodeDefinition } from '@/types/generated/api-types';
import { createYamlAutocompletion } from '@/utils/yamlAutocompletion';

const PaneWrapper = styled.div`
  display: flex;
  flex-direction: column;
  height: 100%;
  overflow: hidden;
  position: absolute;
  top: 0;
  left: 0;
  right: 0;
  bottom: 0;
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
  overflow: hidden;
  padding: 12px;
  display: flex;
  flex-direction: column;
  gap: 10px;
`;

const ErrorBanner = styled.div`
  margin-top: 8px;
  padding: 8px 12px;
  background: var(--sk-error-bg, rgba(239, 68, 68, 0.1));
  border: 1px solid var(--sk-error-border, rgba(239, 68, 68, 0.3));
  border-radius: 4px;
  color: var(--sk-error-text, #ef4444);
  font-size: 12px;
  font-family: var(--sk-font-code);
`;

const CodeMirrorWrapper = styled.div`
  position: relative;
  border: 1px solid var(--sk-border);
  border-radius: 6px;
  overflow: hidden;
  flex: 1;
  display: flex;
  flex-direction: column;

  .cm-editor {
    font-family: var(--sk-font-code);
    font-size: 12px;
    height: 100%;
  }

  .cm-scroller {
    overflow: auto;
    flex: 1;
  }

  .cm-content {
    padding: 8px 0;
  }

  .cm-line {
    padding: 0 10px;
  }

  /* Custom scrollbar styling */
  .cm-scroller::-webkit-scrollbar {
    width: 12px;
    height: 12px;
  }

  .cm-scroller::-webkit-scrollbar-track {
    background: var(--sk-bg);
    border-radius: 6px;
  }

  .cm-scroller::-webkit-scrollbar-thumb {
    background: var(--sk-border);
    border-radius: 6px;
    border: 2px solid var(--sk-bg);

    &:hover {
      background: var(--sk-text-muted);
    }
  }

  .cm-scroller::-webkit-scrollbar-corner {
    background: var(--sk-bg);
  }

  /* Firefox scrollbar styling */
  .cm-scroller {
    scrollbar-width: thin;
    scrollbar-color: var(--sk-border) var(--sk-bg);
  }
`;

interface YamlPaneProps {
  yaml: string;
  onChange?: (yaml: string) => void;
  readOnly?: boolean;
  error?: string;
  nodeDefinitions?: NodeDefinition[];
  highlightNodeLabel?: string;
}

/**
 * Helper function to find the line range of a node in YAML
 * Returns { startLine, endLine } (0-indexed) or null if not found
 */
function findNodeLineRange(
  yaml: string,
  nodeLabel: string
): { startLine: number; endLine: number } | null {
  if (!nodeLabel) return null;

  const lines = yaml.split('\n');
  let inNodesSection = false;
  let nodeStartLine = -1;
  let nodeIndent = -1;

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    const trimmed = line.trim();

    // Check if we're entering the nodes section
    if (trimmed === 'nodes:') {
      inNodesSection = true;
      continue;
    }

    if (!inNodesSection) continue;

    // Check if we've hit a top-level key (end of nodes section)
    if (line.match(/^[a-zA-Z_]/)) {
      // If we found the node, this is the end
      if (nodeStartLine !== -1) {
        return { startLine: nodeStartLine, endLine: i - 1 };
      }
      // Otherwise, we've left the nodes section
      break;
    }

    // Look for the node label as a key
    // Include colons in the pattern to support node names like "transport::moq::peer_1"
    const nodeKeyMatch = line.match(/^(\s+)([a-zA-Z0-9_:.-]+):\s*$/);
    if (nodeKeyMatch) {
      const indent = nodeKeyMatch[1].length;
      const key = nodeKeyMatch[2];

      if (nodeStartLine === -1 && key === nodeLabel) {
        // Found our node
        nodeStartLine = i;
        nodeIndent = indent;
      } else if (nodeStartLine !== -1 && indent === nodeIndent) {
        // Found another node at the same level - this is the end
        return { startLine: nodeStartLine, endLine: i - 1 };
      }
    }
  }

  // If we found the node and reached the end of the file
  if (nodeStartLine !== -1) {
    return { startLine: nodeStartLine, endLine: lines.length - 1 };
  }

  return null;
}

/** Find the array index of an overlay whose YAML `id:` field matches `targetId`
 *  within a given section (e.g. `text_overlays:` or `image_overlays:`).
 *  Returns -1 if not found. */
function findOverlayIndexById(
  lines: string[],
  nodeRange: { startLine: number; endLine: number },
  sectionKey: string,
  targetId: string
): number {
  const section = findSectionStart(lines, nodeRange, sectionKey);
  if (!section) return -1;
  let itemIndex = -1;
  for (let i = section.start + 1; i <= nodeRange.endLine; i++) {
    const line = lines[i];
    const indent = line.length - line.trimStart().length;
    if (indent <= section.indent && line.trim().length > 0) break;
    if (line.trimStart().startsWith('- ') && indent > section.indent) {
      itemIndex++;
    }
    const trimmed = line.trim();
    // Strip leading "- " so we also match "- id: foo" (id on array-item line)
    const stripped = trimmed.startsWith('- ') ? trimmed.slice(2) : trimmed;
    if (
      (stripped === `id: ${targetId}` ||
        stripped === `id: "${targetId}"` ||
        stripped === `id: '${targetId}'`) &&
      itemIndex >= 0
    ) {
      return itemIndex;
    }
  }
  return -1;
}

/** Locate the start line and indent of a YAML section key within a range. */
function findSectionStart(
  lines: string[],
  nodeRange: { startLine: number; endLine: number },
  sectionKey: string
): { start: number; indent: number } | null {
  for (let i = nodeRange.startLine; i <= nodeRange.endLine; i++) {
    if (lines[i].trim() === `${sectionKey}:`) {
      return { start: i, indent: lines[i].length - lines[i].trimStart().length };
    }
  }
  return null;
}

/** Find a map-style video layer key (e.g. "in_0:") under a section. */
function findMapKeyRange(
  lines: string[],
  endLine: number,
  sectionStart: number,
  sectionIndent: number,
  subKey: string
): { startLine: number; endLine: number } | null {
  let keyStart = -1;
  let keyIndent = -1;
  for (let i = sectionStart + 1; i <= endLine; i++) {
    const line = lines[i];
    const indent = line.length - line.trimStart().length;
    if (indent <= sectionIndent && line.trim().length > 0) break;
    const m = line.match(/^(\s+)([a-zA-Z0-9_:.-]+):\s*$/);
    if (m && m[2] === subKey && indent > sectionIndent) {
      keyStart = i;
      keyIndent = indent;
      continue;
    }
    if (keyStart !== -1 && indent <= keyIndent && line.trim().length > 0) {
      return { startLine: keyStart, endLine: i - 1 };
    }
  }
  return keyStart !== -1 ? { startLine: keyStart, endLine } : null;
}

/** Find the Nth array item ("- " prefix) under a section. */
function findArrayItemRange(
  lines: string[],
  endLine: number,
  sectionStart: number,
  sectionIndent: number,
  arrayIndex: number
): { startLine: number; endLine: number } | null {
  let itemCount = -1;
  let itemStart = -1;
  let itemIndent = -1;
  for (let i = sectionStart + 1; i <= endLine; i++) {
    const line = lines[i];
    const indent = line.length - line.trimStart().length;
    if (indent <= sectionIndent && line.trim().length > 0) break;
    if (line.trimStart().startsWith('- ') && indent > sectionIndent) {
      if (itemStart !== -1 && itemCount === arrayIndex) {
        return { startLine: itemStart, endLine: i - 1 };
      }
      itemCount++;
      itemStart = i;
      itemIndent = indent;
    }
  }
  if (itemStart !== -1 && itemCount === arrayIndex) {
    for (let i = itemStart + 1; i <= endLine; i++) {
      const line = lines[i];
      const indent = line.length - line.trimStart().length;
      if (indent <= sectionIndent && line.trim().length > 0)
        return { startLine: itemStart, endLine: i - 1 };
      if (line.trimStart().startsWith('- ') && indent <= itemIndent)
        return { startLine: itemStart, endLine: i - 1 };
    }
    return { startLine: itemStart, endLine };
  }
  return null;
}

/**
 * Find a compositor layer/overlay sub-key within a node's YAML range.
 * Supports video layers (e.g. "in_0" under "layers:") by map-key lookup,
 * and text/image overlays by scanning for a matching `id:` field within
 * the `text_overlays:` / `image_overlays:` YAML arrays.
 */
function findLayerLineRange(
  yaml: string,
  nodeRange: { startLine: number; endLine: number },
  layerId: string
): { startLine: number; endLine: number } | null {
  const lines = yaml.split('\n');

  // Try video layers first (map key under "layers:")
  const layersSection = findSectionStart(lines, nodeRange, 'layers');
  if (layersSection) {
    const mapRange = findMapKeyRange(
      lines,
      nodeRange.endLine,
      layersSection.start,
      layersSection.indent,
      layerId
    );
    if (mapRange) return mapRange;
  }

  // Try text overlays (search by id field)
  const textIdx = findOverlayIndexById(lines, nodeRange, 'text_overlays', layerId);
  if (textIdx >= 0) {
    const textSection = findSectionStart(lines, nodeRange, 'text_overlays');
    if (textSection) {
      return findArrayItemRange(
        lines,
        nodeRange.endLine,
        textSection.start,
        textSection.indent,
        textIdx
      );
    }
  }

  // Try image overlays (search by id field)
  const imgIdx = findOverlayIndexById(lines, nodeRange, 'image_overlays', layerId);
  if (imgIdx >= 0) {
    const imgSection = findSectionStart(lines, nodeRange, 'image_overlays');
    if (imgSection) {
      return findArrayItemRange(
        lines,
        nodeRange.endLine,
        imgSection.start,
        imgSection.indent,
        imgIdx
      );
    }
  }

  return null;
}

/** Debounce delay (ms) for YAML edits in staging mode. */
const YAML_EDIT_DEBOUNCE_MS = 500;

const YamlPane: React.FC<YamlPaneProps> = ({
  yaml,
  onChange,
  readOnly = false,
  error,
  nodeDefinitions = [],
  highlightNodeLabel,
}) => {
  const colorMode = useResolvedColorMode();
  const isDarkMode = colorMode === 'dark';
  const editorViewRef = useRef<EditorView | null>(null);

  // Debounce the upstream onChange so intermediate (invalid) YAML states
  // produced while typing don't trigger a flood of parse-error toasts.
  // CodeMirror manages its own internal buffer, so the editor stays
  // responsive — only the parent's parse/validate cycle is deferred.
  const debouncedOnChange = useMemo(() => {
    if (!onChange) return undefined;
    return debounce(onChange, YAML_EDIT_DEBOUNCE_MS, { leading: false, trailing: true });
  }, [onChange]);

  // Cancel any pending debounced call on unmount or when onChange changes.
  useEffect(() => {
    return () => {
      debouncedOnChange?.cancel();
    };
  }, [debouncedOnChange]);

  const handleChange = useCallback(
    (value: string) => {
      debouncedOnChange?.(value);
    },
    [debouncedOnChange]
  );

  // Create highlighting extension for selected node
  const highlightExtension = useMemo(() => {
    // Define the effect to set highlight range
    const setHighlightEffect = StateEffect.define<{ startLine: number; endLine: number } | null>();

    // Define the state field to store decorations
    const highlightField = StateField.define({
      create() {
        return Decoration.none;
      },
      update(decorations, tr) {
        decorations = decorations.map(tr.changes);
        for (const effect of tr.effects) {
          if (effect.is(setHighlightEffect)) {
            if (effect.value === null) {
              decorations = Decoration.none;
            } else {
              const { startLine, endLine } = effect.value;
              const highlights: Range<Decoration>[] = [];

              // Add decoration for each line in the range
              for (let line = startLine; line <= endLine; line++) {
                const lineObj = tr.state.doc.line(line + 1); // CodeMirror lines are 1-indexed
                highlights.push(
                  Decoration.line({
                    attributes: {
                      class: 'cm-highlighted-node-line',
                    },
                  }).range(lineObj.from)
                );
              }

              decorations = Decoration.set(highlights);
            }
          }
        }
        return decorations;
      },
      provide: (f) => EditorView.decorations.from(f),
    });

    // Custom styling for highlighted lines
    const highlightTheme = EditorView.baseTheme({
      '.cm-highlighted-node-line': {
        backgroundColor: 'rgba(59, 130, 246, 0.1)',
      },
    });

    return [highlightField, highlightTheme, setHighlightEffect];
  }, []);

  // Extract the effect type from the extension
  const setHighlightEffect = highlightExtension[2] as StateEffectType<{
    startLine: number;
    endLine: number;
  } | null>;

  // Read compositor layer selection (published by CompositorNode)
  const compositorSelection = useCompositorSelection();

  // Update highlights when highlightNodeLabel or compositor layer selection changes
  useEffect(() => {
    if (!editorViewRef.current) return;

    // If a compositor layer is selected, drill into that layer's YAML range
    let range: { startLine: number; endLine: number } | null = null;
    const nodeLabel = highlightNodeLabel || '';

    if (
      compositorSelection.layerId &&
      compositorSelection.nodeLabel &&
      compositorSelection.nodeLabel === nodeLabel
    ) {
      const nodeRange = findNodeLineRange(yaml, nodeLabel);
      if (nodeRange) {
        range = findLayerLineRange(yaml, nodeRange, compositorSelection.layerId) ?? nodeRange;
      }
    } else {
      range = findNodeLineRange(yaml, nodeLabel);
    }

    if (range) {
      // Apply highlight and scroll to view
      editorViewRef.current.dispatch({
        effects: setHighlightEffect.of(range),
      });

      // Scroll to the highlighted section
      const startLine = editorViewRef.current.state.doc.line(range.startLine + 1);
      editorViewRef.current.dispatch({
        effects: EditorView.scrollIntoView(startLine.from, {
          y: 'center',
        }),
      });
    } else {
      // Clear highlight if no node selected
      editorViewRef.current.dispatch({
        effects: setHighlightEffect.of(null),
      });
    }
  }, [highlightNodeLabel, yaml, setHighlightEffect, compositorSelection]);

  // Create autocompletion extension with keyboard shortcuts
  const autocompletionExtension = useMemo(() => {
    if (readOnly || nodeDefinitions.length === 0) return [];

    // High-precedence keymap to handle Tab when completion is active
    const tabKeymap = Prec.highest(
      EditorView.domEventHandlers({
        keydown: (event, view) => {
          if (event.key === 'Tab' && !event.shiftKey) {
            const status = completionStatus(view.state);
            if (status === 'active') {
              event.preventDefault();
              acceptCompletion(view);
              return true;
            }
          }
          return false;
        },
      })
    );

    // Keymap for manually triggering autocomplete with Ctrl+Space
    const completionKeymap = keymap.of([
      {
        key: 'Ctrl-Space',
        mac: 'Cmd-Space',
        run: (view) => {
          startCompletion(view);
          return true;
        },
      },
    ]);

    return [createYamlAutocompletion(nodeDefinitions), tabKeymap, completionKeymap];
  }, [readOnly, nodeDefinitions]);

  const basicSetupOptions = useMemo(
    () => ({
      lineNumbers: true,
      highlightActiveLineGutter: !readOnly,
      highlightActiveLine: !readOnly,
      foldGutter: true,
      dropCursor: !readOnly,
      indentOnInput: !readOnly,
      bracketMatching: true,
      closeBrackets: !readOnly,
      autocompletion: !readOnly,
      highlightSelectionMatches: !readOnly,
    }),
    [readOnly]
  );

  const editorExtensions = useMemo(() => {
    const extensions: Extension[] = [
      yamlLang(),
      ...autocompletionExtension,
      highlightExtension[0] as Extension, // highlightField
      highlightExtension[1] as Extension, // highlightTheme
    ];
    if (readOnly) {
      extensions.push(EditorView.editable.of(false));
    }
    return extensions;
  }, [autocompletionExtension, readOnly, highlightExtension]);

  // Capture the EditorView instance when the editor is created
  const onCreateEditor = (view: EditorView) => {
    editorViewRef.current = view;
  };

  return (
    <PaneWrapper data-testid="yaml-pane">
      <PaneHeader>
        <PaneTitle>Pipeline YAML</PaneTitle>
        <PaneSubtitle>
          {readOnly
            ? 'Read-only view'
            : 'Edit pipeline configuration (Ctrl+Space for autocomplete)'}
        </PaneSubtitle>
      </PaneHeader>
      <ContentWrapper>
        <CodeMirrorWrapper>
          <CopyButton text={yaml} />
          <CodeMirror
            value={yaml}
            onChange={handleChange}
            extensions={editorExtensions}
            theme={isDarkMode ? solarizedDark : solarizedLight}
            basicSetup={basicSetupOptions}
            editable={!readOnly}
            readOnly={readOnly}
            height="100%"
            style={{ height: '100%' }}
            onCreateEditor={onCreateEditor}
          />
        </CodeMirrorWrapper>
        {error && <ErrorBanner>{error}</ErrorBanner>}
      </ContentWrapper>
    </PaneWrapper>
  );
};

export default YamlPane;
