import { act, renderHook, waitFor } from '@testing-library/react';
import type { IConfirmation, TMessage } from '@/common/chat/chatLib';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import {
  buildPendingConfirmationMessage,
  PENDING_CONFIRMATION_RECOVERY_INTERVAL_MS,
  usePendingConfirmationsRecovery,
} from '@/renderer/pages/conversation/Messages/usePendingConfirmationsRecovery';

type ConfirmationAddEvent = IConfirmation<unknown> & { conversation_id: string };
type ConfirmationUpdateEvent = IConfirmation<unknown> & { conversation_id: string };
type ConfirmationRemoveEvent = { conversation_id: string; id: string; call_id?: string };

const mocks = vi.hoisted(() => ({
  addHandler: { current: undefined as ((event: ConfirmationAddEvent) => void) | undefined },
  updateHandler: { current: undefined as ((event: ConfirmationUpdateEvent) => void) | undefined },
  removeHandler: { current: undefined as ((event: ConfirmationRemoveEvent) => void) | undefined },
  addOn: vi.fn(),
  updateOn: vi.fn(),
  removeOn: vi.fn(),
  listInvoke: vi.fn(),
  updateMessageList: vi.fn(),
  offAdd: vi.fn(),
  offUpdate: vi.fn(),
  offRemove: vi.fn(),
}));

vi.mock('@/common', () => ({
  ipcBridge: {
    conversation: {
      confirmation: {
        add: { on: mocks.addOn },
        update: { on: mocks.updateOn },
        remove: { on: mocks.removeOn },
        list: { invoke: mocks.listInvoke },
      },
    },
  },
}));

vi.mock('@/renderer/pages/conversation/Messages/hooks', () => ({
  useUpdateMessageList: () => mocks.updateMessageList,
}));

const confirmation: ConfirmationAddEvent = {
  conversation_id: 'conv-1',
  id: 'approval-1',
  call_id: 'call-1',
  title: 'Write file',
  description: 'Write /tmp/current_time.txt',
  command_type: 'edit',
  options: [{ label: 'Allow', value: 'allow_once' }],
};

const latestUpdater = () => {
  const call = mocks.updateMessageList.mock.calls.at(-1);
  expect(call).toBeDefined();
  return call?.[0] as (list: TMessage[]) => TMessage[];
};

describe('usePendingConfirmationsRecovery', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.addHandler.current = undefined;
    mocks.updateHandler.current = undefined;
    mocks.removeHandler.current = undefined;
    mocks.listInvoke.mockResolvedValue([]);
    mocks.addOn.mockImplementation((handler: (event: ConfirmationAddEvent) => void) => {
      mocks.addHandler.current = handler;
      return mocks.offAdd;
    });
    mocks.updateOn.mockImplementation((handler: (event: ConfirmationUpdateEvent) => void) => {
      mocks.updateHandler.current = handler;
      return mocks.offUpdate;
    });
    mocks.removeOn.mockImplementation((handler: (event: ConfirmationRemoveEvent) => void) => {
      mocks.removeHandler.current = handler;
      return mocks.offRemove;
    });
  });

  it('adds live confirmation cards for the active conversation only', async () => {
    renderHook(() => usePendingConfirmationsRecovery('conv-1'));

    await waitFor(() => {
      expect(mocks.addOn).toHaveBeenCalledTimes(1);
    });
    await waitFor(() => {
      expect(mocks.updateMessageList).toHaveBeenCalledTimes(1);
    });
    mocks.updateMessageList.mockClear();

    act(() => {
      mocks.addHandler.current?.({ ...confirmation, conversation_id: 'other-conv' });
    });
    expect(mocks.updateMessageList).not.toHaveBeenCalled();

    act(() => {
      mocks.addHandler.current?.(confirmation);
    });

    const next = latestUpdater()([]);
    expect(next).toHaveLength(1);
    expect(next[0].type).toBe('permission');
    expect(next[0].content.call_id).toBe('call-1');
  });

  it('polls the authoritative pending list to recover a missed add event', async () => {
    vi.useFakeTimers();
    mocks.listInvoke.mockResolvedValueOnce([]).mockResolvedValueOnce([confirmation]);
    const { unmount } = renderHook(() => usePendingConfirmationsRecovery('conv-1'));

    try {
      await act(async () => {
        await Promise.resolve();
      });
      expect(mocks.listInvoke).toHaveBeenCalledTimes(1);
      mocks.updateMessageList.mockClear();

      await act(async () => {
        await vi.advanceTimersByTimeAsync(PENDING_CONFIRMATION_RECOVERY_INTERVAL_MS);
      });

      expect(mocks.listInvoke).toHaveBeenCalledTimes(2);
      const next = latestUpdater()([]);
      expect(next).toHaveLength(1);
      expect(next[0].type).toBe('permission');
      expect(next[0].content.id).toBe('approval-1');
    } finally {
      unmount();
      vi.useRealTimers();
    }
  });

  it('polls the authoritative pending list to remove a missed decision event', async () => {
    vi.useFakeTimers();
    mocks.listInvoke.mockResolvedValueOnce([confirmation]).mockResolvedValueOnce([]);
    const { unmount } = renderHook(() => usePendingConfirmationsRecovery('conv-1'));

    try {
      await act(async () => {
        await Promise.resolve();
      });
      mocks.updateMessageList.mockClear();

      await act(async () => {
        await vi.advanceTimersByTimeAsync(PENDING_CONFIRMATION_RECOVERY_INTERVAL_MS);
      });

      const existing = [buildPendingConfirmationMessage('conv-1', confirmation)] as TMessage[];
      expect(latestUpdater()(existing)).toHaveLength(0);
    } finally {
      unmount();
      vi.useRealTimers();
    }
  });

  it('queues a focus recovery while the initial request is still in flight', async () => {
    let resolveInitial: ((value: IConfirmation<unknown>[]) => void) | undefined;
    mocks.listInvoke
      .mockImplementationOnce(
        () =>
          new Promise<IConfirmation<unknown>[]>((resolve) => {
            resolveInitial = resolve;
          })
      )
      .mockResolvedValueOnce([confirmation]);
    renderHook(() => usePendingConfirmationsRecovery('conv-1'));

    await waitFor(() => expect(mocks.listInvoke).toHaveBeenCalledTimes(1));
    act(() => window.dispatchEvent(new Event('focus')));
    expect(mocks.listInvoke).toHaveBeenCalledTimes(1);

    await act(async () => {
      resolveInitial?.([]);
      await Promise.resolve();
    });

    await waitFor(() => expect(mocks.listInvoke).toHaveBeenCalledTimes(2));
    const next = latestUpdater()([]);
    expect(next).toHaveLength(1);
    expect(next[0].type).toBe('permission');
  });

  it('updates live confirmation cards for the active conversation only', async () => {
    renderHook(() => usePendingConfirmationsRecovery('conv-1'));

    await waitFor(() => {
      expect(mocks.updateOn).toHaveBeenCalledTimes(1);
    });
    await waitFor(() => {
      expect(mocks.updateMessageList).toHaveBeenCalledTimes(1);
    });
    mocks.updateMessageList.mockClear();

    act(() => {
      mocks.updateHandler.current?.({
        ...confirmation,
        conversation_id: 'other-conv',
        title: 'Ignored update',
      });
    });
    expect(mocks.updateMessageList).not.toHaveBeenCalled();

    act(() => {
      mocks.updateHandler.current?.({
        ...confirmation,
        title: 'Updated approval',
        description: 'Updated approval detail',
      });
    });

    const existing = [buildPendingConfirmationMessage('conv-1', confirmation)] as TMessage[];
    const next = latestUpdater()(existing);
    expect(next).toHaveLength(1);
    expect(next[0].type).toBe('permission');
    expect(next[0].content.title).toBe('Updated approval');
    expect(next[0].content.description).toBe('Updated approval detail');
  });

  it('removes live confirmation cards by call_id and unsubscribes on unmount', async () => {
    const { unmount } = renderHook(() => usePendingConfirmationsRecovery('conv-1'));

    await waitFor(() => {
      expect(mocks.removeOn).toHaveBeenCalledTimes(1);
    });

    act(() => {
      mocks.removeHandler.current?.({ conversation_id: 'conv-1', id: 'different-id', call_id: 'call-1' });
    });

    const existing = [buildPendingConfirmationMessage('conv-1', confirmation)] as TMessage[];
    expect(latestUpdater()(existing)).toHaveLength(0);

    unmount();
    expect(mocks.offAdd).toHaveBeenCalledTimes(1);
    expect(mocks.offUpdate).toHaveBeenCalledTimes(1);
    expect(mocks.offRemove).toHaveBeenCalledTimes(1);
  });
});
