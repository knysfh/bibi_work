import { render, screen, waitFor } from '@testing-library/react';
import type { IConfirmation } from '@/common/chat/chatLib';
import React from 'react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { MessageListProvider, useMessageList } from '@/renderer/pages/conversation/Messages/hooks';
import { usePendingConfirmationsRecovery } from '@/renderer/pages/conversation/Messages/usePendingConfirmationsRecovery';

const mocks = vi.hoisted(() => ({
  listInvoke: vi.fn(),
}));

vi.mock('@/common', () => ({
  ipcBridge: {
    conversation: {
      confirmation: {
        add: { on: () => () => undefined },
        update: { on: () => () => undefined },
        remove: { on: () => () => undefined },
        list: { invoke: mocks.listInvoke },
      },
    },
  },
}));

const confirmation: IConfirmation<unknown> = {
  id: 'approval-1',
  call_id: 'call-1',
  title: 'Approve write',
  action: 'exec',
  options: [{ label: 'Allow once', value: 'proceed_once' }],
};

const Probe = () => {
  usePendingConfirmationsRecovery('conv-1');
  const messages = useMessageList();
  return <div data-testid='permission-count'>{messages.filter((message) => message.type === 'permission').length}</div>;
};

describe('pending confirmation recovery provider integration', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.listInvoke.mockResolvedValue([confirmation]);
  });

  it('writes recovered confirmations into the mounted message-list provider', async () => {
    render(
      <MessageListProvider value={[]}>
        <Probe />
      </MessageListProvider>
    );

    await waitFor(() => expect(screen.getByTestId('permission-count')).toHaveTextContent('1'));
  });
});
