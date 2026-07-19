import { describe, expect, it } from 'vitest';
import { normalizeToolCall } from './normalizeToolCall';

describe('normalizeToolCall', () => {
  it('ignores tool_call messages without call_id', () => {
    const result = normalizeToolCall({
      type: 'tool_call',
      content: {
        call_id: '',
        name: 'Glob',
        status: 'running',
        args: { pattern: '*.rs' },
      },
    } as any);

    expect(result).toBeUndefined();
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
