// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import js from "@eslint/js";
import globals from "globals";
import tseslint from "typescript-eslint";
import reactHooks from "eslint-plugin-react-hooks";
// @ts-expect-error - eslint-plugin-import doesn't have proper types for flat config yet
import importPlugin from "eslint-plugin-import";
import tsParser from "@typescript-eslint/parser";
import sonarjs from "eslint-plugin-sonarjs";
import complexityPlugin from "eslint-plugin-complexity";

export default tseslint.config(
  js.configs.recommended,
  ...tseslint.configs.recommended,
  importPlugin.flatConfigs.recommended,
  importPlugin.flatConfigs.typescript,
  {
    files: ["**/*.{js,mjs,cjs,ts,tsx,mts,cts}"],
    plugins: {
      "react-hooks": reactHooks,
      sonarjs,
      complexity: complexityPlugin,
    },
    languageOptions: {
      globals: globals.browser,
      parserOptions: {
        ecmaVersion: "latest",
        sourceType: "module",
      },
    },
    settings: {
      "import/resolver": {
        typescript: {
          alwaysTryTypes: true,
          project: "./tsconfig.json",
        },
      },
    },
    rules: {
      // React Hooks
      "react-hooks/rules-of-hooks": "error",
      "react-hooks/exhaustive-deps": "warn",

      // Code style
      quotes: [
        "error",
        "single",
        { avoidEscape: true, allowTemplateLiterals: true },
      ],

      // Console usage
      "no-console": ["warn", { allow: ["warn", "error"] }],

      // Import sorting and organization
      "import/order": [
        "error",
        {
          groups: [
            ["builtin", "external"],  // Node.js built-ins and npm packages together
            "internal",                // @/ aliases
            ["parent", "sibling", "index"],  // Relative imports together
          ],
          pathGroups: [
            {
              pattern: "@/**",
              group: "internal",
              position: "before",
            },
          ],
          pathGroupsExcludedImportTypes: [],
          "newlines-between": "always",
          alphabetize: {
            order: "asc",
            caseInsensitive: true,
          },
          warnOnUnassignedImports: false,
        },
      ],
      "import/newline-after-import": "error",
      "import/no-duplicates": "error",
      "import/first": "error",

      // ---- Core complexity limits ----
      "complexity": ["warn", 15],                 // cyclomatic complexity
      "sonarjs/cognitive-complexity": ["warn", 30],

      // ---- File size / nesting heuristics ----
      "max-lines": ["warn", { max: 500, skipBlankLines: true, skipComments: true }],
      "max-depth": ["warn", 4],                    // nesting depth
      "max-statements": ["warn", 30],

      // ---- React-specific structure heuristics (optional) ----
      // Often useful for identifying big components
      "sonarjs/no-identical-functions": "warn",
      // Disabled: CSS variables, styled-components design tokens, and console prefixes
      // are legitimate duplications that don't benefit from extraction.
      // Use code review to catch actual problematic string duplication.
      "sonarjs/no-duplicate-string": "off",
      "sonarjs/no-nested-switch": "warn",
      "sonarjs/no-nested-template-literals": "warn",
    },
  },
  {
    ignores: ["src/types/generated/**"],
  },
  {
    // Exception for large View components that are inherently complex orchestration layers
    // These coordinate multiple sub-components, manage complex state, and handle many events.
    // Focus on breaking down their internal functions rather than splitting the files.
    files: [
      "**/views/MonitorView.tsx",   // 2818 lines - Real-time monitoring with complex state sync
      "**/views/DesignView.tsx",    // 1382 lines - Visual pipeline editor
      "**/views/ConvertView.tsx",   // 1158 lines - File conversion UI with progress tracking
      "**/views/StreamView.tsx",    // 673 lines - Live streaming with MoQ connection orchestration
      "**/panes/ControlPane.tsx",   // 868 lines - Control panel with many widgets
      "**/panes/SamplePipelinesPane.tsx", // 582 lines - Pipeline templates browser
    ],
    rules: {
      "max-lines": "off",  // Acknowledged: these are complex view orchestrators
      // View components have inherent complexity from orchestrating state, events, and UI
      "complexity": "off",
    },
  },
  {
    // Exception for streamStore connect() method - requires sequential initialization of
    // multiple interdependent MoQ resources (connection -> watch/publish -> microphone/emitter).
    // Splitting would obscure the initialization order and error handling flow.
    files: ["**/stores/streamStore.ts"],
    rules: {
      "max-statements": "off",
    },
  },
  {
    // Exception for deepEqual utility - intentionally uses early-exit branches for performance.
    // Each branch handles a distinct type (primitive, array, object) with minimal overhead.
    files: ["**/utils/deepEqual.ts"],
    rules: {
      "complexity": "off",
    },
  },
);
