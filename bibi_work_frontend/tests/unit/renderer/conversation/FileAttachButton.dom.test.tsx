/**
 * @license
 * Copyright 2025 AionUi (aionui.com)
 * SPDX-License-Identifier: Apache-2.0
 */

import React from 'react';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const { conversationUpdate, mutateConversation, selectedServerIds } = vi.hoisted(() => ({
  conversationUpdate: vi.fn(),
  mutateConversation: vi.fn(),
  selectedServerIds: { current: [] as string[] },
}));

vi.mock('react-i18next', () => ({
  useTranslation: () => ({
    t: (key: string, options?: { defaultValue?: string }) => options?.defaultValue ?? key,
  }),
}));

vi.mock('react-router-dom', () => ({
  useNavigate: () => vi.fn(),
}));

vi.mock('@/common', () => ({
  ipcBridge: {
    conversation: {
      update: {
        invoke: conversationUpdate,
      },
    },
    fs: {
      listAvailableSkills: {
        invoke: vi.fn().mockResolvedValue([]),
      },
    },
  },
}));

vi.mock('@/common/adapter/ipcBridge', () => ({
  mcpService: {
    listServers: {
      invoke: vi.fn().mockResolvedValue([
        {
          id: 'mcp-universal',
          name: 'universal',
          enabled: true,
          builtin: false,
          health_status: 'healthy',
          transport: { type: 'streamable_http', url: 'http://example.invalid/mcp' },
        },
      ]),
    },
  },
}));

vi.mock('@/renderer/hooks/context/ConversationContext', () => ({
  useConversationContextSafe: () => ({
    conversation_id: 'conversation-1',
    type: 'acp',
    selectedMcpServerIds: selectedServerIds.current,
  }),
}));

vi.mock('@/renderer/utils/platform', () => ({
  isElectronDesktop: () => true,
}));

vi.mock('@/renderer/services/FileService', () => ({
  FileService: {
    processDroppedFiles: vi.fn().mockResolvedValue([]),
  },
}));

vi.mock('@/renderer/utils/emitter', () => ({
  emitter: { emit: vi.fn() },
}));

vi.mock('swr', () => ({
  default: (key: string | null) => ({
    data:
      key === 'conversation-mcp-server-catalog'
        ? [
            {
              id: 'mcp-universal',
              name: 'universal',
              enabled: true,
              builtin: false,
              health_status: 'healthy',
              transport: { type: 'streamable_http', url: 'http://example.invalid/mcp' },
            },
          ]
        : undefined,
    isLoading: false,
  }),
  useSWRConfig: () => ({ mutate: mutateConversation }),
}));

vi.mock('@icon-park/react', () => ({
  FolderOpen: () => null,
  Lightning: () => null,
  Paperclip: () => null,
  Plus: () => null,
  Right: () => null,
  Shield: () => null,
}));

vi.mock('@arco-design/web-react', () => ({
  Button: ({
    children,
    loading: _loading,
    icon: _icon,
    shape: _shape,
    type: _type,
    ...props
  }: React.ButtonHTMLAttributes<HTMLButtonElement> & {
    loading?: boolean;
    icon?: React.ReactNode;
    shape?: string;
    type?: string;
  }) => <button {...props}>{children}</button>,
  Checkbox: ({ checked, disabled }: { checked?: boolean; disabled?: boolean }) => (
    <input type='checkbox' checked={checked} disabled={disabled} readOnly />
  ),
  Message: { error: vi.fn() },
  Spin: () => <span>loading</span>,
  Trigger: ({ children, popup }: { children: React.ReactNode; popup: () => React.ReactNode }) => (
    <>
      {children}
      {popup()}
    </>
  ),
}));

import FileAttachButton from '@/renderer/components/media/FileAttachButton';

describe('FileAttachButton conversation MCP selection', () => {
  beforeEach(() => {
    selectedServerIds.current = [];
    conversationUpdate.mockReset();
    conversationUpdate.mockResolvedValue(true);
    mutateConversation.mockReset();
    mutateConversation.mockResolvedValue(undefined);
  });

  it('persists a selected MCP server in the conversation metadata', async () => {
    render(<FileAttachButton openFileSelector={vi.fn()} />);

    fireEvent.click(screen.getByTestId('conversation-mcp-server-mcp-universal'));

    await waitFor(() => {
      expect(conversationUpdate).toHaveBeenCalledWith({
        id: 'conversation-1',
        updates: {
          extra: {
            selected_mcp_server_ids: ['mcp-universal'],
            mcp_server_ids: ['mcp-universal'],
          },
        },
        merge_extra: true,
      });
    });
    expect(mutateConversation).toHaveBeenCalledWith('conversation/conversation-1');
  });

  it('removes an already selected MCP server', async () => {
    selectedServerIds.current = ['mcp-universal'];
    render(<FileAttachButton openFileSelector={vi.fn()} />);

    fireEvent.click(screen.getByTestId('conversation-mcp-server-mcp-universal'));

    await waitFor(() => {
      expect(conversationUpdate).toHaveBeenCalledWith(
        expect.objectContaining({
          updates: {
            extra: {
              selected_mcp_server_ids: [],
              mcp_server_ids: [],
            },
          },
        })
      );
    });
  });
});
