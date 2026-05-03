// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { Logger, type ILogObj } from 'tslog';

const getDefaultLogLevel = (): number => {
  // Vite sets MODE to "production" for production builds
  return import.meta.env.MODE === 'production' ? 3 : 2;
};

export function getLogger(name: string): Logger<ILogObj> {
  return new Logger({
    name,
    minLevel: getDefaultLogLevel(),
    type: 'pretty',
    prettyLogTemplate: '{{logLevelName}} [{{name}}]: ',
    stylePrettyLogs: false, // Disable colors for better visibility on all console backgrounds
    hideLogPositionForProduction: true, // Performance: disable code position gathering in production
  });
}

// Convenience loggers for common modules
export const viewsLogger = getLogger('views');
export const componentsLogger = getLogger('components');
export const hooksLogger = getLogger('hooks');
export const nodesLogger = getLogger('nodes');
export const utilsLogger = getLogger('utils');
