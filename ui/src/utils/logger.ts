// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { Logger, type ILogObj } from 'tslog';

/**
 * Determine the default log level based on environment
 * - Production: 3 (info - shows info, warn, error)
 * - Development: 2 (debug - shows all levels)
 *
 * tslog log levels: 0=silly, 1=trace, 2=debug, 3=info, 4=warn, 5=error, 6=fatal
 */
const getDefaultLogLevel = (): number => {
  // Vite sets MODE to "production" for production builds
  return import.meta.env.MODE === 'production' ? 3 : 2;
};

/**
 * Create a named logger instance with consistent configuration
 * @param name - Logger name (typically module or file name)
 * @returns Configured logger instance
 */
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
export const servicesLogger = getLogger('services');
export const viewsLogger = getLogger('views');
export const componentsLogger = getLogger('components');
export const storesLogger = getLogger('stores');
export const hooksLogger = getLogger('hooks');
export const panesLogger = getLogger('panes');
export const nodesLogger = getLogger('nodes');
export const utilsLogger = getLogger('utils');
