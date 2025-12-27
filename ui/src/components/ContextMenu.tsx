// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import React from 'react';

import { Menu, MenuItem } from './Menu';

interface ContextMenuProps {
  id: string;
  top?: number | false;
  left?: number | false;
  right?: number | false;
  bottom?: number | false;
  onDuplicate: (nodeId: string) => void;
  onDelete: (nodeId: string) => void;
  onClick: () => void;
}

const ContextMenu: React.FC<ContextMenuProps> = ({
  id,
  onDuplicate,
  onDelete,
  onClick,
  ...props
}) => {
  return (
    <Menu onClose={onClick} {...props}>
      <MenuItem
        onClick={() => {
          onDuplicate(id);
          onClick();
        }}
      >
        Duplicate
      </MenuItem>
      <MenuItem
        danger
        onClick={() => {
          onDelete(id);
          onClick();
        }}
      >
        Delete
      </MenuItem>
    </Menu>
  );
};

export default ContextMenu;
