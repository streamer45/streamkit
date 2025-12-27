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

interface SaveTemplateModalProps {
  isOpen: boolean;
  onClose: () => void;
  onSave: (name: string, description: string, overwrite?: boolean) => Promise<void>;
  mode?: 'oneshot' | 'dynamic';
  initialName?: string;
  initialDescription?: string;
}

const SaveTemplateModal: React.FC<SaveTemplateModalProps> = ({
  isOpen,
  onClose,
  onSave,
  mode = 'dynamic',
  initialName = '',
  initialDescription = '',
}) => {
  const [name, setName] = useState(initialName);
  const [description, setDescription] = useState(initialDescription);
  const [isSaving, setIsSaving] = useState(false);
  const [showOverwriteConfirm, setShowOverwriteConfirm] = useState(false);

  // Update name and description when initial values change
  React.useEffect(() => {
    if (isOpen) {
      setName(initialName);
      setDescription(initialDescription);
    }
  }, [isOpen, initialName, initialDescription]);

  const handleSave = async (overwrite = false) => {
    if (!name.trim()) return;

    setIsSaving(true);
    try {
      await onSave(name.trim(), description.trim(), overwrite);
      setName('');
      setDescription('');
      setShowOverwriteConfirm(false);
      onClose();
    } catch (error) {
      // Check if it's a 409 conflict error (duplicate name)
      if (error instanceof Error && error.message.includes('409')) {
        setShowOverwriteConfirm(true);
      } else {
        throw error;
      }
    } finally {
      setIsSaving(false);
    }
  };

  const handleOverwriteConfirm = () => {
    setShowOverwriteConfirm(false);
    handleSave(true);
  };

  const handleOverwriteCancel = () => {
    setShowOverwriteConfirm(false);
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Enter' && name.trim() && !isSaving) {
      e.preventDefault();
      handleSave();
    }
  };

  return (
    <>
      <Dialog
        open={isOpen && !showOverwriteConfirm}
        onOpenChange={(open) => !open && !isSaving && onClose()}
      >
        <DialogPortal>
          <DialogOverlay />
          <DialogContent>
            <DialogHeader>
              <DialogTitle>
                Save {mode === 'oneshot' ? '📄 Oneshot' : '⚡ Dynamic'} Template
              </DialogTitle>
            </DialogHeader>
            <DialogBody>
              <FormGroup>
                <Label htmlFor="template-name">Template Name *</Label>
                <Input
                  id="template-name"
                  type="text"
                  placeholder="Enter template name"
                  value={name}
                  onChange={(e) => setName(e.target.value)}
                  onKeyDown={handleKeyDown}
                  disabled={isSaving}
                  maxLength={50}
                  autoFocus
                />
              </FormGroup>
              <FormGroup>
                <Label htmlFor="template-description">Description</Label>
                <Input
                  id="template-description"
                  type="text"
                  placeholder="Optional description"
                  value={description}
                  onChange={(e) => setDescription(e.target.value)}
                  onKeyDown={handleKeyDown}
                  disabled={isSaving}
                  maxLength={200}
                />
              </FormGroup>
            </DialogBody>
            <DialogFooter>
              <DialogClose asChild>
                <Button variant="ghost" onClick={onClose} disabled={isSaving}>
                  Cancel
                </Button>
              </DialogClose>
              <Button
                variant="primary"
                onClick={() => handleSave(false)}
                disabled={!name.trim() || isSaving}
              >
                {isSaving ? 'Saving...' : 'Save'}
              </Button>
            </DialogFooter>
          </DialogContent>
        </DialogPortal>
      </Dialog>

      <Dialog open={showOverwriteConfirm} onOpenChange={(open) => !open && handleOverwriteCancel()}>
        <DialogPortal>
          <DialogOverlay />
          <DialogContent>
            <DialogHeader>
              <DialogTitle>Overwrite Existing Template?</DialogTitle>
            </DialogHeader>
            <DialogBody>
              <p>
                A template named <strong>"{name}"</strong> already exists. Do you want to overwrite
                it?
              </p>
            </DialogBody>
            <DialogFooter>
              <Button variant="ghost" onClick={handleOverwriteCancel} disabled={isSaving}>
                Cancel
              </Button>
              <Button variant="danger" onClick={handleOverwriteConfirm} disabled={isSaving}>
                {isSaving ? 'Overwriting...' : 'Overwrite'}
              </Button>
            </DialogFooter>
          </DialogContent>
        </DialogPortal>
      </Dialog>
    </>
  );
};

export default SaveTemplateModal;
