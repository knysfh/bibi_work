import { describe, expect, it } from 'vitest';
import type { IMessageAcpToolCall } from '@/common/chat/chatLib';
import { normalizeAcpToolCall, normalizeToolCall } from '@/common/chat/normalizeToolCall';

describe('normalizeToolCall', () => {
  it('normalizes compact snake_case acp tool calls from history responses', () => {
    const result = normalizeAcpToolCall({
      id: 'message-1',
      conversation_id: 'conversation-1',
      type: 'acp_tool_call',
      content: {
        _compact: {
          truncated: true,
          original_size: 90000,
          preview_chars: 4096,
        },
        update: {
          session_update: 'tool_call',
          tool_call_id: 'tool-1',
          status: 'completed',
          title: 'rg',
          kind: 'search',
          raw_input: { pattern: 'needle', path: '.' },
          content: [{ type: 'content', content: { type: 'text', text: 'preview' } }],
        },
      },
    } as unknown as IMessageAcpToolCall);

    expect(result).toMatchObject({
      key: 'tool-1',
      name: 'rg',
      status: 'completed',
      description: '"needle" in .',
      output: 'preview',
      truncated: true,
      messageId: 'message-1',
      conversationId: 'conversation-1',
      inputFields: [
        { key: 'pattern', label: 'Pattern', value: 'needle', sensitive: false },
        { key: 'path', label: 'Path', value: '.', sensitive: false },
      ],
    });
  });

  it('renders escaped Unicode tool summaries as UTF-8 text', () => {
    const result = normalizeToolCall({
      type: 'tool_call',
      content: {
        call_id: 'unicode-summary',
        name: 'read_file',
        status: 'completed',
        output: '\\u4e2d\\u6587\\u5de5\\u5177\\u6458\\u8981',
      },
    } as any);

    expect(result?.output).toBe('中文工具摘要');
    expect(result?.output).not.toContain('\\u');
  });
});
