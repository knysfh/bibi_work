/**
 * @license
 * Copyright 2025 AionUi (aionui.com)
 * SPDX-License-Identifier: Apache-2.0
 */

import { act, renderHook, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { HubStateChange } from '@/common/types/agent/hub';

const onStateChangedHandlers: Array<(payload: HubStateChange) => void> = [];

vi.mock('swr', () => ({
  mutate: vi.fn(),
}));

vi.mock('@/common', () => ({
  ipcBridge: {
    hub: {
      getExtensionList: {
        invoke: vi.fn().mockResolvedValue([
          {
            name: 'agent-a',
            display_name: 'Agent A',
            description: 'demo',
            status: 'not_installed',
            hubs: ['acpAdapters'],
          },
        ]),
      },
      onStateChanged: {
        on: vi.fn((handler) => {
          onStateChangedHandlers.push(handler);
          return vi.fn();
        }),
      },
      install: {
        invoke: vi.fn().mockResolvedValue({
          name: 'agent-a',
          status: 'install_failed',
          error: 'Local hub extension installer is not attached.',
        }),
      },
      retryInstall: {
        invoke: vi.fn().mockResolvedValue({
          name: 'agent-a',
          status: 'install_failed',
          error: 'Local hub extension installer is not attached.',
        }),
      },
      update: { invoke: vi.fn().mockResolvedValue({ name: 'agent-a', status: 'installed' }) },
    },
  },
}));

import { ipcBridge } from '@/common';
import { mutate } from 'swr';
import { useHubAgents } from '@/renderer/hooks/agent/useHubAgents';

describe('useHubAgents', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    onStateChangedHandlers.length = 0;
  });

  it('refreshes managed-agent and assistant caches when an install completes', async () => {
    renderHook(() => useHubAgents());

    await waitFor(() => {
      expect(onStateChangedHandlers).toHaveLength(1);
    });

    await act(async () => {
      onStateChangedHandlers[0]({ name: 'agent-a', status: 'installed' });
    });

    expect(mutate).toHaveBeenCalledWith('agents.managed');
    expect(mutate).toHaveBeenCalledWith('assistants.list');
    expect(mutate).toHaveBeenCalledWith('assistants');
  });

  it('applies hub mutation responses that carry extension governance sync summaries', async () => {
    vi.mocked(ipcBridge.hub.install.invoke).mockResolvedValueOnce({
      name: 'agent-a',
      status: 'install_failed',
      error: 'Local hub extension installer is not attached.',
      governanceSync: { synced: 1, contributions: 2 },
    });
    const { result } = renderHook(() => useHubAgents());

    await waitFor(() => {
      expect(result.current.agents).toHaveLength(1);
    });

    await act(async () => {
      await result.current.install('agent-a');
    });

    expect(ipcBridge.hub.install.invoke).toHaveBeenCalledWith({ name: 'agent-a' });
    expect(result.current.agents[0]).toMatchObject({
      name: 'agent-a',
      status: 'install_failed',
      installError: 'Local hub extension installer is not attached.',
    });
  });
});
