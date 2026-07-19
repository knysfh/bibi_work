import type { IConfirmation, TMessage } from '@/common/chat/chatLib';
import { describe, expect, it } from 'vitest';
import {
  appendPendingConfirmationMessage,
  buildPendingConfirmationMessage,
  hasPermissionMessageForCallId,
  hasPermissionMessageForConfirmation,
  reconcilePendingConfirmationMessages,
  removePermissionMessage,
  upsertPendingConfirmationMessage,
} from '@/renderer/pages/conversation/Messages/usePendingConfirmationsRecovery';

const confirmation: IConfirmation<string> = {
  id: 'tool-1',
  call_id: 'tool-1',
  title: 'Write file',
  description: 'Write /tmp/current_time.txt',
  command_type: 'edit',
  options: [{ label: 'Allow', value: 'allow_once' }],
};

describe('pending confirmations recovery', () => {
  it('builds a permission message with stable msg_id from confirmation id', () => {
    const message = buildPendingConfirmationMessage('conv-1', confirmation);

    expect(message.type).toBe('permission');
    expect(message.conversation_id).toBe('conv-1');
    expect(message.msg_id).toBe('confirmation:tool-1');
    expect(message.content.call_id).toBe('tool-1');
  });

  it('detects existing permission messages by call_id', () => {
    const list = [buildPendingConfirmationMessage('conv-1', confirmation)];

    expect(hasPermissionMessageForCallId(list, 'tool-1')).toBe(true);
    expect(hasPermissionMessageForCallId(list, 'tool-2')).toBe(false);
    expect(hasPermissionMessageForConfirmation(list, { ...confirmation, id: 'different-id' })).toBe(true);
  });

  it('appends pending confirmations once by confirmation id or call_id', () => {
    const first = appendPendingConfirmationMessage([], 'conv-1', confirmation);
    const duplicateCallId = appendPendingConfirmationMessage(first, 'conv-1', { ...confirmation, id: 'tool-2' });
    const duplicateId = appendPendingConfirmationMessage(first, 'conv-1', { ...confirmation, call_id: 'tool-2' });

    expect(first).toHaveLength(1);
    expect(duplicateCallId).toBe(first);
    expect(duplicateId).toBe(first);
  });

  it('upserts pending confirmations by confirmation id or call_id', () => {
    const first = upsertPendingConfirmationMessage([], 'conv-1', confirmation);
    const updated = upsertPendingConfirmationMessage(first, 'conv-1', {
      ...confirmation,
      title: 'Updated write file',
      description: 'Updated description',
    });

    expect(updated).toHaveLength(1);
    expect(updated[0].type).toBe('permission');
    expect(updated[0].content.title).toBe('Updated write file');
    expect(updated[0].content.description).toBe('Updated description');
  });

  it('removes recovered permission messages by confirmation id or call_id', () => {
    const list = [
      buildPendingConfirmationMessage('conv-1', confirmation),
      { id: 'text-1', type: 'text', conversation_id: 'conv-1', content: { content: 'hello' } },
    ] as TMessage[];

    const result = removePermissionMessage(list, { id: 'other-id', call_id: 'tool-1' });

    expect(result).toHaveLength(1);
    expect(result[0].type).toBe('text');
  });

  it('reconciles stale permission messages against the authoritative pending list', () => {
    const stale = { ...confirmation, id: 'stale-approval', call_id: 'stale-call' };
    const list = [
      buildPendingConfirmationMessage('conv-1', stale),
      buildPendingConfirmationMessage('other-conv', stale),
      { id: 'text-1', type: 'text', conversation_id: 'conv-1', content: { content: 'hello' } },
    ] as TMessage[];

    const result = reconcilePendingConfirmationMessages(list, 'conv-1', [confirmation]);

    expect(result.filter((message) => message.type === 'permission' && message.conversation_id === 'conv-1')).toEqual([
      expect.objectContaining({ content: expect.objectContaining({ id: 'tool-1', call_id: 'tool-1' }) }),
    ]);
    expect(result.some((message) => message.type === 'permission' && message.conversation_id === 'other-conv')).toBe(
      true
    );
    expect(result.some((message) => message.type === 'text')).toBe(true);
  });
});
