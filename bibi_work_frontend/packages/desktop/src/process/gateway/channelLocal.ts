/**
 * @license
 * Copyright 2025 AionUi (aionui.com)
 * SPDX-License-Identifier: Apache-2.0
 */

export type ChannelPluginTestResult = {
  success: boolean;
  bot_username?: string;
  error?: string;
};

export class ChannelLocalRouteError extends Error {
  constructor(
    public readonly statusCode: number,
    public readonly code: string,
    message: string
  ) {
    super(message);
    this.name = 'ChannelLocalRouteError';
  }
}

const CONNECTOR_RUNTIME_NOT_ATTACHED_ERROR = 'Local channel connector runtime is not attached.';

function requiredString(body: Record<string, unknown>, key: string): string {
  const value = body[key];
  if (typeof value !== 'string' || !value.trim()) {
    throw new ChannelLocalRouteError(400, 'INVALID_INPUT', `${key} is required`);
  }
  return value.trim();
}

export async function handleChannelLocalRoute(pathname: string, body: Record<string, unknown>): Promise<unknown> {
  switch (pathname) {
    case '/api/channel/plugins/test': {
      const pluginId = requiredString(body, 'plugin_id');
      requiredString(body, 'token');
      return {
        success: false,
        error: `${CONNECTOR_RUNTIME_NOT_ATTACHED_ERROR} (${pluginId})`,
      } satisfies ChannelPluginTestResult;
    }
    default:
      throw new ChannelLocalRouteError(404, 'CHANNEL_LOCAL_ROUTE_NOT_FOUND', 'desktop local channel route not found');
  }
}
