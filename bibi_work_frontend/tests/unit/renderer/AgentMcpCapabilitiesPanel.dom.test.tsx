/**
 * @license
 * Copyright 2025 AionUi (aionui.com)
 * SPDX-License-Identifier: Apache-2.0
 */

import React from 'react';
import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { Message } from '@arco-design/web-react';
import { acpConversation, type AgentMcpCapabilities } from '@/common/adapter/ipcBridge';
import AgentMcpCapabilitiesPanel from '@/renderer/pages/settings/AgentSettings/AgentMcpCapabilitiesPanel';

vi.mock('@/common/adapter/ipcBridge', () => ({
  acpConversation: {
    getAgentMcpCapabilities: { invoke: vi.fn() },
    publishAgentMcpCapabilities: { invoke: vi.fn() },
  },
}));

vi.mock('react-i18next', () => ({
  useTranslation: () => ({
    t: (key: string) => key,
  }),
}));

const capabilities = (selectedIds: string[], browserEnabled = false): AgentMcpCapabilities => ({
  agent_id: 'agent-1',
  agent_version_id: 'version-1',
  browser_enabled: browserEnabled,
  selected_mcp_tool_ids: selectedIds,
  stale_mcp_tool_ids: [],
  servers: [
    {
      id: 'server-1',
      name: 'universal',
      tools: [
        {
          id: 'tool-read',
          name: 'read_rows',
          description: 'Read database rows',
          risk_level: 'low',
          risk_source: 'server_annotation',
          read_only: true,
          destructive: false,
          selected: selectedIds.includes('tool-read'),
          stale: false,
        },
        {
          id: 'tool-status',
          name: 'get_connection_status',
          description: 'Get connection status',
          risk_level: 'medium',
          risk_source: 'name_heuristic',
          read_only: false,
          destructive: false,
          selected: selectedIds.includes('tool-status'),
          stale: false,
        },
      ],
    },
  ],
});

describe('AgentMcpCapabilitiesPanel', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.spyOn(Message, 'success').mockImplementation(() => undefined as never);
  });

  it('loads the published whitelist and publishes a precise per-tool selection', async () => {
    const user = userEvent.setup();
    const getMock = vi.mocked(acpConversation.getAgentMcpCapabilities.invoke);
    const publishMock = vi.mocked(acpConversation.publishAgentMcpCapabilities.invoke);
    const onPublished = vi.fn();
    getMock
      .mockResolvedValueOnce(capabilities(['tool-read']))
      .mockResolvedValueOnce(capabilities(['tool-read', 'tool-status']));
    publishMock.mockResolvedValue({
      changed: true,
      agent_id: 'agent-1',
      agent_version_id: 'version-2',
      browser_enabled: false,
      selected_mcp_tool_ids: ['tool-read', 'tool-status'],
      previous_version_revoked: false,
    });

    render(<AgentMcpCapabilitiesPanel agentId='agent-1' onPublished={onPublished} />);

    await waitFor(() => expect(getMock).toHaveBeenCalledWith({ id: 'agent-1' }));
    const readCheckbox = within(await screen.findByTestId('agent-mcp-tool-tool-read')).getByRole('checkbox');
    const statusCheckbox = within(screen.getByTestId('agent-mcp-tool-tool-status')).getByRole('checkbox');
    expect(readCheckbox).toBeChecked();
    expect(statusCheckbox).not.toBeChecked();

    await user.click(statusCheckbox);
    await user.click(screen.getByRole('button', { name: 'settings.agentMcp.publish' }));

    await waitFor(() => {
      expect(publishMock).toHaveBeenCalledWith({
        id: 'agent-1',
        mcp_tool_ids: ['tool-read', 'tool-status'],
        browser_enabled: false,
      });
      expect(onPublished).toHaveBeenCalledTimes(1);
      expect(Message.success).toHaveBeenCalledWith('settings.agentMcp.published');
    });
  });

  it('can reduce the selection to tools explicitly annotated read-only', async () => {
    const user = userEvent.setup();
    vi.mocked(acpConversation.getAgentMcpCapabilities.invoke).mockResolvedValue(
      capabilities(['tool-read', 'tool-status'])
    );

    render(<AgentMcpCapabilitiesPanel agentId='agent-1' />);

    const readCheckbox = within(await screen.findByTestId('agent-mcp-tool-tool-read')).getByRole('checkbox');
    const statusCheckbox = within(screen.getByTestId('agent-mcp-tool-tool-status')).getByRole('checkbox');
    expect(readCheckbox).toBeChecked();
    expect(statusCheckbox).toBeChecked();
    await user.click(screen.getByRole('button', { name: 'settings.agentMcp.selectReadOnly' }));
    expect(readCheckbox).toBeChecked();
    expect(statusCheckbox).not.toBeChecked();
  });

  it('exposes the local browser capability and publishes its disabled state', async () => {
    const user = userEvent.setup();
    const getMock = vi.mocked(acpConversation.getAgentMcpCapabilities.invoke);
    const publishMock = vi.mocked(acpConversation.publishAgentMcpCapabilities.invoke);
    getMock.mockResolvedValueOnce(capabilities([], true)).mockResolvedValueOnce(capabilities([], false));
    publishMock.mockResolvedValue({
      changed: true,
      agent_id: 'agent-1',
      agent_version_id: 'version-2',
      browser_enabled: false,
      selected_mcp_tool_ids: [],
      previous_version_revoked: false,
    });

    render(<AgentMcpCapabilitiesPanel agentId='agent-1' />);

    const browserCheckbox = within(await screen.findByTestId('agent-browser-capability')).getByRole('checkbox');
    expect(browserCheckbox).toBeChecked();
    await user.click(browserCheckbox);
    await user.click(screen.getByRole('button', { name: 'settings.agentMcp.publish' }));

    await waitFor(() => {
      expect(publishMock).toHaveBeenCalledWith({
        id: 'agent-1',
        mcp_tool_ids: [],
        browser_enabled: false,
      });
    });
  });
});
