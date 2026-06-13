// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { useState } from 'react';

let childRenders = 0;

export const getChildRenders = () => childRenders;
export const resetChildRenders = () => {
  childRenders = 0;
};

function Child({ label }: { label: string }) {
  childRenders++;
  return <span>{label}</span>;
}

export function Parent() {
  const [count, setCount] = useState(0);
  return (
    <div>
      <button onClick={() => setCount(count + 1)}>{count}</button>
      <Child label="stable" />
    </div>
  );
}
