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

interface CreateSessionModalProps {
  isOpen: boolean;
  onClose: () => void;
  onCreate: (name: string) => Promise<void>;
  mode?: 'oneshot' | 'dynamic';
}

const CreateSessionModal: React.FC<CreateSessionModalProps> = ({
  isOpen,
  onClose,
  onCreate,
  mode = 'dynamic',
}) => {
  const [name, setName] = useState('');
  const [isCreating, setIsCreating] = useState(false);

  const handleCreate = async () => {
    setIsCreating(true);
    try {
      await onCreate(name.trim());
      setName('');
      onClose();
    } finally {
      setIsCreating(false);
    }
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Enter' && !isCreating) {
      e.preventDefault();
      handleCreate();
    }
  };

  return (
    <Dialog open={isOpen} onOpenChange={(open) => !open && !isCreating && onClose()}>
      <DialogPortal>
        <DialogOverlay />
        <DialogContent>
          <DialogHeader>
            <DialogTitle>
              Create {mode === 'oneshot' ? '📄 Oneshot' : '⚡ Dynamic'} Session
            </DialogTitle>
          </DialogHeader>
          <DialogBody>
            <FormGroup>
              <Label htmlFor="session-name">Session Name (optional)</Label>
              <Input
                id="session-name"
                type="text"
                placeholder="Enter session name"
                value={name}
                onChange={(e) => setName(e.target.value)}
                onKeyDown={handleKeyDown}
                disabled={isCreating}
                maxLength={50}
                autoFocus
              />
            </FormGroup>
          </DialogBody>
          <DialogFooter>
            <DialogClose asChild>
              <Button variant="ghost" onClick={onClose} disabled={isCreating}>
                Cancel
              </Button>
            </DialogClose>
            <Button variant="primary" onClick={handleCreate} disabled={isCreating}>
              {isCreating ? 'Creating...' : 'Create'}
            </Button>
          </DialogFooter>
        </DialogContent>
      </DialogPortal>
    </Dialog>
  );
};

export default CreateSessionModal;
