<!--
SPDX-FileCopyrightText: © 2025 StreamKit Contributors

SPDX-License-Identifier: MPL-2.0
-->

# StreamKit UI Color System

All colors for the StreamKit UI are centralized in two files:

## Files

### 1. `index.css` (Primary Color Definitions)

The main CSS file (`ui/src/index.css`) defines all color variables using CSS custom properties. This is the **single source of truth** for all colors in the application.

**Color Categories:**

- **Base colors**: Background, sidebar, panel, text
- **Borders**: Border and border-strong
- **Primary colors**: Brand colors with contrast variants
- **Semantic colors**: Success, warning, danger, info
- **Status indicators**: Node state colors (initializing, running, recovering, degraded, failed, stopped)
- **UI accents**: Indigo palette for buttons and highlights
- **Overlays**: Transparent overlays for hover states
- **Text variations**: White and light text colors

**Theme Support:**
All colors are defined for both light and dark themes using an optimized approach:

- **Default**: Light theme in `:root`
- **Auto dark**: `@media (prefers-color-scheme: dark)` with `:root:not([data-skit-theme="light"])`
- **Explicit dark**: `:root[data-skit-theme="dark"]` override
- **Explicit light**: Uses default `:root` values (no separate definition needed)

### 2. `colors.ts` (Programmatic Access)

TypeScript utility file for accessing colors in JavaScript/TypeScript code. Use this when you need to:

- Pass colors to third-party libraries that don't support CSS variables
- Generate dynamic styles based on theme
- Access colors in JavaScript logic

## Usage

### In Styled Components (Preferred)

Always use CSS variables directly in styled-components:

```typescript
import styled from '@emotion/styled';

const StyledDiv = styled.div`
  color: var(--sk-text);
  background: var(--sk-panel-bg);
  border: 1px solid var(--sk-border);
`;
```

### In Inline Styles

```typescript
<div style={{ color: 'var(--sk-text)', background: 'var(--sk-panel-bg)' }}>
  Content
</div>
```

### In TypeScript/JavaScript (When Necessary)

```typescript
import { getColor, getStatusColor } from '@/styles/colors';

// Get a specific color value
const primaryColor = getColor('primary');

// Get a status color
const runningColor = getStatusColor('Running');
```

## Available Color Variables

### Base Colors

- `--sk-bg`: Main background
- `--sk-sidebar-bg`: Sidebar background
- `--sk-panel-bg`: Panel background
- `--sk-text`: Primary text
- `--sk-text-muted`: Muted/secondary text
- `--sk-text-white`: White text
- `--sk-text-light`: Light text

### Borders

- `--sk-border`: Standard border
- `--sk-border-strong`: Emphasized border

### Brand/Primary

- `--sk-primary`: Primary brand color
- `--sk-primary-contrast`: Contrasting color for primary

### Semantic Colors

- `--sk-success`: Success/positive actions
- `--sk-warning`: Warning/caution
- `--sk-danger`: Error/destructive actions
- `--sk-info`: Informational
- `--sk-muted`: Muted/disabled state

### Status Indicators

- `--sk-status-initializing`: Node is starting up
- `--sk-status-running`: Node is operating normally
- `--sk-status-recovering`: Node is recovering from an issue
- `--sk-status-degraded`: Node is operational but degraded
- `--sk-status-failed`: Node has failed
- `--sk-status-stopped`: Node is stopped

### UI Accents

- `--sk-accent-indigo`: Primary accent (indigo)
- `--sk-accent-indigo-light`: Lighter indigo
- `--sk-accent-indigo-dark`: Darker indigo

### Interactive States

- `--sk-hover-bg`: Hover background

### Overlays

- `--sk-overlay-light`: Light transparent overlay
- `--sk-overlay-medium`: Medium transparent overlay
- `--sk-overlay-strong`: Strong transparent overlay

### Effects

- `--sk-shadow`: Box shadow color
- `--sk-focus-ring`: Focus ring style

## Updating Colors

To update the color palette:

1. **Edit `ui/src/index.css`**
2. Update colors in **two locations only**:
   - `:root` (light theme default)
   - `@media (prefers-color-scheme: dark)` **AND** `:root[data-skit-theme="dark"]` (both need the same dark colors)
3. Test both light and dark themes
4. No need to update individual components - they automatically use the new colors

**Note**: You must update both the media query and the explicit dark theme selector with identical values to ensure consistent dark mode appearance regardless of how it's activated.

## Best Practices

1. **Never hardcode colors** - Always use CSS variables
2. **Use semantic names** - Use `--sk-success` instead of `--sk-green`
3. **Maintain contrast** - Ensure adequate contrast between text and backgrounds
4. **Test both themes** - Always verify changes in light and dark modes
5. **Prefer CSS variables** - Only use `colors.ts` when CSS variables aren't supported
