/**
 * @license
 * Copyright 2025 AionUi (aionui.com)
 * SPDX-License-Identifier: Apache-2.0
 */

import { describe, expect, it } from 'vitest';
import {
  handleChannelLocalRoute,
  ChannelLocalRouteError,
  type ChannelPluginTestResult,
} from '@process/gateway/channelLocal';

async function route(pathname: string, body: Record<string, unknown> = {}): Promise<unknown> {
  return handleChannelLocalRoute(pathname, body);
}

describe('desktop channel local routes', () => {
  it('handles connector dry-run locally when runtime is not attached', async () => {
    const result = (await route('/api/channel/plugins/test', {
      plugin_id: 'telegram',
      token: '123456:ABCDEF',
    })) as ChannelPluginTestResult;

    expect(result).toEqual({
      success: false,
      error: 'Local channel connector runtime is not attached. (telegram)',
    });
  });

  it('validates required dry-run fields', async () => {
    const promise = route('/api/channel/plugins/test', { plugin_id: 'telegram' });

    await expect(promise).rejects.toBeInstanceOf(ChannelLocalRouteError);
    await expect(promise).rejects.toMatchObject({
      statusCode: 400,
      code: 'INVALID_INPUT',
    });
  });
});
