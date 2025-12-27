// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import React, { createContext, useContext, useState } from 'react';
import type { ReactNode } from 'react';

interface DnDContextType {
  type: string | null;
  setType: (type: string | null) => void;
}

const DnDContext = createContext<DnDContextType | undefined>(undefined);

interface DnDProviderProps {
  children: ReactNode;
}

export const DnDProvider: React.FC<DnDProviderProps> = ({ children }) => {
  const [type, setType] = useState<string | null>(null);

  return <DnDContext.Provider value={{ type, setType }}>{children}</DnDContext.Provider>;
};

export const useDnD = (): [string | null, (type: string | null) => void] => {
  const context = useContext(DnDContext);
  if (context === undefined) {
    throw new Error('useDnD must be used within a DnDProvider');
  }
  return [context.type, context.setType];
};
