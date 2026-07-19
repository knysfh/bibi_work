/**
 * @license
 * Copyright 2025 AionUi (aionui.com)
 * SPDX-License-Identifier: Apache-2.0
 */

import { ipcBridge } from '@/common';
import type { IConfirmation, IMessagePermission, TMessage } from '@/common/chat/chatLib';
import { useUpdateMessageList } from '@renderer/pages/conversation/Messages/hooks';
import { useEffect } from 'react';

export const PENDING_CONFIRMATION_RECOVERY_INTERVAL_MS = 3_000;

export const pendingConfirmationMsgId = (confirmationId: string) => `confirmation:${confirmationId}`;

export function buildPendingConfirmationMessage(
  conversation_id: string,
  confirmation: IConfirmation<unknown>
): IMessagePermission {
  return {
    id: pendingConfirmationMsgId(confirmation.id),
    msg_id: pendingConfirmationMsgId(confirmation.id),
    type: 'permission',
    position: 'left',
    conversation_id,
    created_at: Date.now(),
    content: confirmation,
  };
}

export function hasPermissionMessageForCallId(list: TMessage[], callId: string): boolean {
  return list.some((message) => message.type === 'permission' && message.content?.call_id === callId);
}

export function hasPermissionMessageForConfirmation(list: TMessage[], confirmation: IConfirmation<unknown>): boolean {
  return list.some(
    (message) =>
      message.type === 'permission' &&
      (message.content?.id === confirmation.id || message.content?.call_id === confirmation.call_id)
  );
}

export function appendPendingConfirmationMessage(
  list: TMessage[],
  conversation_id: string,
  confirmation: IConfirmation<unknown>
): TMessage[] {
  if (hasPermissionMessageForConfirmation(list, confirmation)) return list;
  return list.concat(buildPendingConfirmationMessage(conversation_id, confirmation));
}

export function upsertPendingConfirmationMessage(
  list: TMessage[],
  conversation_id: string,
  confirmation: IConfirmation<unknown>
): TMessage[] {
  const index = list.findIndex(
    (message) =>
      message.type === 'permission' &&
      (message.content?.id === confirmation.id || message.content?.call_id === confirmation.call_id)
  );
  if (index < 0) return list.concat(buildPendingConfirmationMessage(conversation_id, confirmation));

  const next = list.slice();
  const existing = next[index];
  if (existing.type !== 'permission') return list;
  next[index] = {
    ...existing,
    content: { ...existing.content, ...confirmation },
  };
  return next;
}

export function removePermissionMessage(list: TMessage[], target: { id?: string; call_id?: string }): TMessage[] {
  return list.filter((message) => {
    if (message.type !== 'permission') return true;
    if (target.id && message.content.id === target.id) return false;
    if (target.call_id && message.content.call_id === target.call_id) return false;
    return true;
  });
}

export function reconcilePendingConfirmationMessages(
  list: TMessage[],
  conversation_id: string,
  confirmations: IConfirmation<unknown>[]
): TMessage[] {
  const pendingIds = new Set(confirmations.map((confirmation) => confirmation.id));
  const pendingCallIds = new Set(confirmations.map((confirmation) => confirmation.call_id));
  let next = list.filter(
    (message) =>
      message.type !== 'permission' ||
      message.conversation_id !== conversation_id ||
      pendingIds.has(message.content.id) ||
      pendingCallIds.has(message.content.call_id)
  );
  for (const confirmation of confirmations) {
    next = upsertPendingConfirmationMessage(next, conversation_id, confirmation);
  }
  return next;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function usePendingConfirmationsRecovery(conversation_id: string) {
  const updateMessageList = useUpdateMessageList();

  useEffect(() => {
    if (!conversation_id) return;
    let cancelled = false;
    let recovering = false;
    let recoveryQueued = false;

    const recoverPendingConfirmations = () => {
      if (cancelled) return;
      if (recovering) {
        recoveryQueued = true;
        return;
      }
      recovering = true;
      void ipcBridge.conversation.confirmation.list
        .invoke({ conversation_id })
        .then((confirmations) => {
          if (cancelled) return;
          updateMessageList((list) => reconcilePendingConfirmationMessages(list, conversation_id, confirmations ?? []));
        })
        .catch((error) => {
          console.warn('[pending-confirmations] failed to recover pending confirmations', {
            conversation_id,
            error: errorMessage(error),
          });
        })
        .finally(() => {
          recovering = false;
          if (recoveryQueued && !cancelled) {
            recoveryQueued = false;
            recoverPendingConfirmations();
          }
        });
    };
    recoverPendingConfirmations();
    const recoveryTimer = window.setInterval(recoverPendingConfirmations, PENDING_CONFIRMATION_RECOVERY_INTERVAL_MS);
    const recoverOnFocus = () => recoverPendingConfirmations();
    const recoverWhenVisible = () => {
      if (document.visibilityState === 'visible') recoverPendingConfirmations();
    };
    window.addEventListener('focus', recoverOnFocus);
    document.addEventListener('visibilitychange', recoverWhenVisible);

    const offAdd = ipcBridge.conversation.confirmation.add.on((event) => {
      if (event.conversation_id !== conversation_id) return;
      updateMessageList((list) => upsertPendingConfirmationMessage(list, conversation_id, event));
    });

    const offUpdate = ipcBridge.conversation.confirmation.update.on((event) => {
      if (event.conversation_id !== conversation_id) return;
      updateMessageList((list) => upsertPendingConfirmationMessage(list, conversation_id, event));
    });

    const offRemove = ipcBridge.conversation.confirmation.remove.on((event) => {
      if (event.conversation_id !== conversation_id) return;
      updateMessageList((list) => removePermissionMessage(list, { id: event.id, call_id: event.call_id ?? event.id }));
    });

    return () => {
      cancelled = true;
      window.clearInterval(recoveryTimer);
      window.removeEventListener('focus', recoverOnFocus);
      document.removeEventListener('visibilitychange', recoverWhenVisible);
      offAdd();
      offUpdate();
      offRemove();
    };
  }, [conversation_id, updateMessageList]);
}
