// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import React, { forwardRef, useCallback, useImperativeHandle, useRef, useState } from 'react';

export type UploadDropZoneHandle = {
  open: () => void;
  reset: () => void;
};

export interface UploadDropZoneProps {
  accept?: string;
  multiple?: boolean;
  disabled?: boolean;
  icon?: React.ReactNode;
  text: React.ReactNode;
  hint?: React.ReactNode;
  onFilesSelected: (files: FileList) => void;
  className?: string;
}

const ZoneButton = styled.button<{ $isDragging: boolean }>`
  width: 100%;
  padding: 24px;
  border: 2px dashed
    ${({ $isDragging }) => ($isDragging ? 'var(--sk-primary)' : 'var(--sk-border)')};
  border-radius: 8px;
  text-align: center;
  background: ${({ $isDragging }) =>
    $isDragging ? 'var(--sk-primary-alpha)' : 'var(--sk-panel-bg)'};
  cursor: pointer;
  transition: all 0.2s;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 8px;
  box-sizing: border-box;
  color: var(--sk-text-muted);

  &:hover {
    border-color: var(--sk-primary);
    background: var(--sk-primary-alpha);
  }

  &:disabled {
    opacity: 0.55;
    cursor: not-allowed;
  }
`;

const IconWrapper = styled.div`
  display: flex;
  align-items: center;
  justify-content: center;
  line-height: 0;
  color: var(--sk-text-muted);
`;

const ZoneText = styled.div`
  font-size: 12px;
  color: var(--sk-text-muted);
`;

const ZoneHint = styled.div`
  font-size: 11px;
  color: var(--sk-text-muted);
`;

const HiddenFileInput = styled.input`
  display: none;
`;

export const UploadDropZone = forwardRef<UploadDropZoneHandle, UploadDropZoneProps>(
  (
    { accept, multiple = false, disabled = false, icon, text, hint, onFilesSelected, className },
    ref
  ) => {
    const inputRef = useRef<HTMLInputElement | null>(null);
    const [isDragging, setIsDragging] = useState(false);

    const open = useCallback(() => {
      if (disabled) return;
      inputRef.current?.click();
    }, [disabled]);

    const reset = useCallback(() => {
      if (inputRef.current) {
        inputRef.current.value = '';
      }
    }, []);

    useImperativeHandle(ref, () => ({ open, reset }), [open, reset]);

    const handleDragOver = useCallback(
      (e: React.DragEvent) => {
        e.preventDefault();
        e.stopPropagation();
        if (disabled) return;
        setIsDragging(true);
      },
      [disabled]
    );

    const handleDragLeave = useCallback((e: React.DragEvent) => {
      e.preventDefault();
      e.stopPropagation();
      setIsDragging(false);
    }, []);

    const handleDrop = useCallback(
      (e: React.DragEvent) => {
        e.preventDefault();
        e.stopPropagation();
        setIsDragging(false);
        if (disabled) return;

        const files = e.dataTransfer.files;
        if (!files || files.length === 0) return;
        onFilesSelected(files);
      },
      [disabled, onFilesSelected]
    );

    const handleInputChange = useCallback(
      (e: React.ChangeEvent<HTMLInputElement>) => {
        const files = e.target.files;
        if (files && files.length > 0) {
          onFilesSelected(files);
        }
        e.target.value = '';
      },
      [onFilesSelected]
    );

    return (
      <>
        <HiddenFileInput
          ref={inputRef}
          type="file"
          accept={accept}
          multiple={multiple}
          onChange={handleInputChange}
          disabled={disabled}
        />
        <ZoneButton
          type="button"
          className={className}
          disabled={disabled}
          $isDragging={isDragging}
          onClick={open}
          onDragOver={handleDragOver}
          onDragLeave={handleDragLeave}
          onDrop={handleDrop}
        >
          {icon ? <IconWrapper>{icon}</IconWrapper> : null}
          <ZoneText>{text}</ZoneText>
          {hint ? <ZoneHint>{hint}</ZoneHint> : null}
        </ZoneButton>
      </>
    );
  }
);

UploadDropZone.displayName = 'UploadDropZone';
