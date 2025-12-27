// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { acceptCompletion, completionStatus, startCompletion } from '@codemirror/autocomplete';
import { yaml } from '@codemirror/lang-yaml';
import { Prec } from '@codemirror/state';
import { EditorView, keymap } from '@codemirror/view';
import styled from '@emotion/styled';
import { solarizedDark, solarizedLight } from '@uiw/codemirror-theme-solarized';
import CodeMirror from '@uiw/react-codemirror';
import React, { useState, useRef, useCallback, useEffect, useMemo } from 'react';

import { CopyButton } from '@/components/CopyButton';
import { useResolvedColorMode } from '@/hooks/useResolvedColorMode';
import type { NodeDefinition } from '@/types/generated/api-types';
import { createYamlAutocompletion } from '@/utils/yamlAutocompletion';

const EditorContainer = styled.div`
  width: 100%;
  display: flex;
  flex-direction: column;
`;

const EditorLabel = styled.label`
  display: block;
  font-weight: 600;
  color: var(--sk-text);
  margin-bottom: 8px;
  font-size: 14px;
`;

const ResizableContainer = styled.div`
  position: relative;
  display: flex;
  flex-direction: column;
`;

const CodeMirrorContainer = styled.div<{ height?: number }>`
  position: relative;
  ${(props) => (props.height ? `height: ${props.height}px;` : '')}
  border: 1px solid var(--sk-border);
  border-bottom: none;
  border-radius: 6px 6px 0 0;
  overflow: hidden;
  transition: border-color 0.2s ease;

  &:focus-within {
    border-color: var(--sk-primary);
  }

  .cm-editor {
    ${(props) => (props.height ? 'height: 100%;' : '')}
    font-family: var(--sk-font-code);
    font-size: 13px;
  }

  .cm-scroller {
    overflow: auto;
  }

  .cm-content {
    padding: 8px 0;
  }

  .cm-line {
    padding: 0 12px;
  }

  /* Custom scrollbar styling for CodeMirror */
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

const ResizeHandle = styled.div<{ isDragging: boolean }>`
  height: 32px;
  width: 100%;
  background: ${(props) => (props.isDragging ? 'var(--sk-primary)' : 'var(--sk-panel-bg)')};
  border: 1px solid var(--sk-border);
  border-top: none;
  border-radius: 0 0 6px 6px;
  cursor: ns-resize;
  display: flex;
  align-items: center;
  justify-content: center;
  transition: background 0.2s ease;
  user-select: none;
  box-sizing: border-box;

  &:hover {
    background: var(--sk-hover-bg);
  }

  &::before {
    content: '';
    width: 40px;
    height: 4px;
    background: ${(props) =>
      props.isDragging ? 'var(--sk-primary-contrast)' : 'var(--sk-border)'};
    border-radius: 2px;
    transition: background 0.2s ease;
  }

  &:hover::before {
    background: var(--sk-primary);
  }
`;

const EditorHint = styled.div`
  margin-top: 8px;
  font-size: 12px;
  color: var(--sk-text-muted);
`;

interface PipelineEditorProps {
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
  nodeDefinitions?: NodeDefinition[];
}

export const PipelineEditor: React.FC<PipelineEditorProps> = ({
  value,
  onChange,
  nodeDefinitions = [],
}) => {
  const [height, setHeight] = useState<number | undefined>(undefined);
  const [isDragging, setIsDragging] = useState(false);
  const dragStartY = useRef<number>(0);
  const dragStartHeight = useRef<number>(0);
  const colorMode = useResolvedColorMode();
  const isDarkMode = colorMode === 'dark';

  const handleChange = (newValue: string) => {
    onChange(newValue);
  };

  // Create autocompletion extension with keyboard shortcuts
  const autocompletionExtension = useMemo(() => {
    if (nodeDefinitions.length === 0) return [];

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
  }, [nodeDefinitions]);

  const handleMouseDown = useCallback(
    (e: React.MouseEvent) => {
      e.preventDefault();
      setIsDragging(true);
      dragStartY.current = e.clientY;
      // Capture the actual height from the element if not set
      const container = (e.target as HTMLElement).previousElementSibling;
      if (container) {
        dragStartHeight.current = height || container.getBoundingClientRect().height;
      }
    },
    [height]
  );

  const handleMouseMove = useCallback(
    (e: MouseEvent) => {
      if (!isDragging) return;

      const deltaY = e.clientY - dragStartY.current;
      const newHeight = Math.max(100, dragStartHeight.current + deltaY);
      setHeight(newHeight);
    },
    [isDragging]
  );

  const handleMouseUp = useCallback(() => {
    setIsDragging(false);
  }, []);

  useEffect(() => {
    if (isDragging) {
      document.addEventListener('mousemove', handleMouseMove);
      document.addEventListener('mouseup', handleMouseUp);
      return () => {
        document.removeEventListener('mousemove', handleMouseMove);
        document.removeEventListener('mouseup', handleMouseUp);
      };
    }
  }, [isDragging, handleMouseMove, handleMouseUp]);

  return (
    <EditorContainer>
      <EditorLabel htmlFor="pipeline-editor">Pipeline Configuration (YAML)</EditorLabel>
      <ResizableContainer>
        <CodeMirrorContainer height={height}>
          <CopyButton text={value} />
          <CodeMirror
            value={value}
            {...(height ? { height: `${height}px` } : {})}
            extensions={[yaml(), ...autocompletionExtension]}
            onChange={handleChange}
            theme={isDarkMode ? solarizedDark : solarizedLight}
            basicSetup={{
              lineNumbers: true,
              highlightActiveLineGutter: true,
              highlightActiveLine: true,
              foldGutter: true,
              dropCursor: true,
              indentOnInput: true,
              bracketMatching: true,
              closeBrackets: true,
              autocompletion: true,
              highlightSelectionMatches: true,
            }}
          />
        </CodeMirrorContainer>
        <ResizeHandle isDragging={isDragging} onMouseDown={handleMouseDown} />
      </ResizableContainer>
      <EditorHint>
        All oneshot pipelines must include both streamkit::http_input and streamkit::http_output
        nodes
      </EditorHint>
    </EditorContainer>
  );
};
