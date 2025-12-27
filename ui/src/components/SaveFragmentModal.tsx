// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import React, { useState } from 'react';

import { Button } from '@/components/ui/Button';
import {
  Dialog,
  DialogBody,
  DialogClose,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogOverlay,
  DialogPortal,
  DialogTitle,
  FormGroup,
  Input,
  Label,
} from '@/components/ui/Dialog';
import { componentsLogger } from '@/utils/logger';

interface SaveFragmentModalProps {
  isOpen: boolean;
  onClose: () => void;
  onSave: (name: string, description: string, tags: string[]) => void;
  initialName?: string;
  initialDescription?: string;
  initialTags?: string[];
}

const SaveFragmentModal: React.FC<SaveFragmentModalProps> = ({
  isOpen,
  onClose,
  onSave,
  initialName = '',
  initialDescription = '',
  initialTags = [],
}) => {
  const [name, setName] = useState('');
  const [description, setDescription] = useState('');
  const [tagsInput, setTagsInput] = useState('');
  const [isSaving, setIsSaving] = useState(false);
  const inputRef = React.useRef<HTMLInputElement>(null);
  const prevOpenRef = React.useRef(false);

  // Update fields only when modal transitions from closed to open
  React.useEffect(() => {
    if (isOpen && !prevOpenRef.current) {
      // Modal just opened
      setName(initialName);
      setDescription(initialDescription);
      setTagsInput(initialTags.join(', '));
      // Manually focus the input after a small delay to ensure modal is fully rendered
      setTimeout(() => {
        inputRef.current?.focus();
      }, 100);
    }
    prevOpenRef.current = isOpen;
  }, [isOpen, initialName, initialDescription, initialTags]);

  const handleSave = async () => {
    if (!name.trim()) return;

    setIsSaving(true);
    try {
      // Parse tags from comma-separated input
      const tags = tagsInput
        .split(',')
        .map((tag) => tag.trim())
        .filter((tag) => tag.length > 0);

      onSave(name.trim(), description.trim(), tags);
      setName('');
      setDescription('');
      setTagsInput('');
      onClose();
    } catch (error) {
      componentsLogger.error('Error saving fragment:', error);
      // Don't close modal on error
    } finally {
      setIsSaving(false);
    }
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Enter' && name.trim() && !isSaving) {
      e.preventDefault();
      handleSave();
    }
  };

  return (
    <Dialog open={isOpen} onOpenChange={(open) => !open && !isSaving && onClose()}>
      <DialogPortal>
        <DialogOverlay />
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Save Pipeline Fragment</DialogTitle>
          </DialogHeader>
          <DialogBody>
            <FormGroup>
              <Label htmlFor="fragment-name">Fragment Name *</Label>
              <Input
                ref={inputRef}
                id="fragment-name"
                type="text"
                placeholder="e.g., Audio Decoder Chain"
                value={name}
                onChange={(e) => setName(e.target.value)}
                onKeyDown={handleKeyDown}
                disabled={isSaving}
                maxLength={50}
              />
            </FormGroup>
            <FormGroup>
              <Label htmlFor="fragment-description">Description</Label>
              <Input
                id="fragment-description"
                type="text"
                placeholder="What does this fragment do?"
                value={description}
                onChange={(e) => setDescription(e.target.value)}
                onKeyDown={handleKeyDown}
                disabled={isSaving}
                maxLength={200}
              />
            </FormGroup>
            <FormGroup>
              <Label htmlFor="fragment-tags">Tags (comma-separated)</Label>
              <Input
                id="fragment-tags"
                type="text"
                placeholder="e.g., audio, decoder, opus"
                value={tagsInput}
                onChange={(e) => setTagsInput(e.target.value)}
                onKeyDown={handleKeyDown}
                disabled={isSaving}
                maxLength={100}
              />
            </FormGroup>
          </DialogBody>
          <DialogFooter>
            <DialogClose asChild>
              <Button variant="ghost" onClick={onClose} disabled={isSaving}>
                Cancel
              </Button>
            </DialogClose>
            <Button variant="primary" onClick={handleSave} disabled={!name.trim() || isSaving}>
              {isSaving ? 'Saving...' : 'Save Fragment'}
            </Button>
          </DialogFooter>
        </DialogContent>
      </DialogPortal>
    </Dialog>
  );
};

export default SaveFragmentModal;
