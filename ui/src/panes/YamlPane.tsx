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
import React, { useMemo, useRef, useEffect } from 'react';

import { CopyButton } from '@/components/CopyButton';
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

  // Update highlights when highlightNodeLabel changes
  useEffect(() => {
    if (!editorViewRef.current) return;

    const range = findNodeLineRange(yaml, highlightNodeLabel || '');

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
  }, [highlightNodeLabel, yaml, setHighlightEffect]);

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
            onChange={onChange}
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
        {error && (
          <div
            style={{
              marginTop: '8px',
              padding: '8px 12px',
              background: 'var(--sk-error-bg, rgba(239, 68, 68, 0.1))',
              border: '1px solid var(--sk-error-border, rgba(239, 68, 68, 0.3))',
              borderRadius: '4px',
              color: 'var(--sk-error-text, #ef4444)',
              fontSize: '12px',
              fontFamily: 'var(--sk-font-code)',
            }}
          >
            {error}
          </div>
        )}
      </ContentWrapper>
    </PaneWrapper>
  );
};

export default YamlPane;
