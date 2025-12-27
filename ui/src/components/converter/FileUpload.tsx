// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import React, { useCallback, useState } from 'react';

const UploadContainer = styled.div`
  width: 100%;
`;

const DropZone = styled.div<{ isDragActive: boolean; hasFile: boolean }>`
  border: 2px dashed ${(props) => (props.isDragActive ? 'var(--sk-primary)' : 'var(--sk-border)')};
  border-radius: 8px;
  padding: 40px 20px;
  text-align: center;
  background: ${(props) => (props.isDragActive ? 'var(--sk-hover-bg)' : 'transparent')};
  cursor: pointer;

  &:hover {
    border-color: var(--sk-primary);
    background: var(--sk-hover-bg);
  }
`;

const UploadIcon = styled.div`
  font-size: 48px;
  margin-bottom: 16px;
  color: var(--sk-text-muted);
`;

const UploadText = styled.div`
  color: var(--sk-text);
  font-size: 16px;
  margin-bottom: 8px;
  font-weight: 500;
`;

const UploadHint = styled.div`
  color: var(--sk-text-muted);
  font-size: 14px;
  margin-top: 4px;
`;

const FileInfo = styled.div`
  margin-top: 16px;
  padding: 16px;
  background: var(--sk-panel-bg);
  border-radius: 6px;
  border: 1px solid var(--sk-border);
  display: flex;
  align-items: center;
  justify-content: space-between;
`;

const FileDetails = styled.div`
  display: flex;
  align-items: center;
  gap: 12px;
`;

const FileName = styled.div`
  color: var(--sk-text);
  font-weight: 500;
`;

const FileSize = styled.div`
  color: var(--sk-text-muted);
  font-size: 14px;
`;

const RemoveButton = styled.button`
  padding: 6px 12px;
  background: transparent;
  color: var(--sk-text);
  border: 1px solid var(--sk-border);
  border-radius: 4px;
  cursor: pointer;
  font-size: 14px;

  &:hover {
    background: var(--sk-hover-bg);
    border-color: var(--sk-primary);
  }
`;

const HiddenFileInput = styled.input`
  display: none;
`;

interface FileUploadProps {
  file: File | null;
  onFileSelect: (file: File | null) => void;
  accept?: string;
}

export const FileUpload: React.FC<FileUploadProps> = ({
  file,
  onFileSelect,
  accept = 'audio/*,.ogg,.opus,.mp3,.wav,.flac',
}) => {
  const [isDragActive, setIsDragActive] = useState(false);
  const fileInputRef = React.useRef<HTMLInputElement>(null);

  const handleDrag = useCallback((e: React.DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
  }, []);

  const handleDragIn = useCallback((e: React.DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
    if (e.dataTransfer.items && e.dataTransfer.items.length > 0) {
      setIsDragActive(true);
    }
  }, []);

  const handleDragOut = useCallback((e: React.DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
    setIsDragActive(false);
  }, []);

  const handleDrop = useCallback(
    (e: React.DragEvent) => {
      e.preventDefault();
      e.stopPropagation();
      setIsDragActive(false);

      if (e.dataTransfer.files && e.dataTransfer.files.length > 0) {
        const droppedFile = e.dataTransfer.files[0];
        onFileSelect(droppedFile);
      }
    },
    [onFileSelect]
  );

  const handleClick = () => {
    fileInputRef.current?.click();
  };

  const handleFileChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    if (e.target.files && e.target.files.length > 0) {
      onFileSelect(e.target.files[0]);
    }
  };

  const handleRemove = (e: React.MouseEvent) => {
    e.stopPropagation();
    onFileSelect(null);
    if (fileInputRef.current) {
      fileInputRef.current.value = '';
    }
  };

  const formatFileSize = (bytes: number): string => {
    if (bytes === 0) return '0 Bytes';
    const k = 1024;
    const sizes = ['Bytes', 'KB', 'MB', 'GB'];
    const i = Math.floor(Math.log(bytes) / Math.log(k));
    return Math.round((bytes / Math.pow(k, i)) * 100) / 100 + ' ' + sizes[i];
  };

  return (
    <UploadContainer>
      <DropZone
        isDragActive={isDragActive}
        hasFile={!!file}
        onClick={handleClick}
        onDragEnter={handleDragIn}
        onDragLeave={handleDragOut}
        onDragOver={handleDrag}
        onDrop={handleDrop}
      >
        <UploadIcon>📁</UploadIcon>
        <UploadText>
          {isDragActive ? 'Drop audio file here' : 'Click to upload or drag and drop'}
        </UploadText>
        <UploadHint>Supports audio files (Ogg/Opus, MP3, WAV, FLAC)</UploadHint>
      </DropZone>

      <HiddenFileInput ref={fileInputRef} type="file" accept={accept} onChange={handleFileChange} />

      {file && (
        <FileInfo>
          <FileDetails>
            <div style={{ fontSize: '24px' }}>🎵</div>
            <div>
              <FileName>{file.name}</FileName>
              <FileSize>{formatFileSize(file.size)}</FileSize>
            </div>
          </FileDetails>
          <RemoveButton onClick={handleRemove}>Remove</RemoveButton>
        </FileInfo>
      )}
    </UploadContainer>
  );
};
