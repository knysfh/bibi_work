import React from 'react';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { IMessageAcpToolCall, IMessagePermission, IMessageToolGroup } from '@/common/chat/chatLib';
import MessagePermission from '@/renderer/pages/conversation/Messages/components/MessagePermission';
import MessageToolGroup from '@/renderer/pages/conversation/Messages/components/MessageToolGroup';
import MessageToolGroupSummary from '@/renderer/pages/conversation/Messages/components/MessageToolGroupSummary';

vi.mock('react-i18next', () => ({
  useTranslation: () => ({ t: (key: string, options?: { defaultValue?: string }) => options?.defaultValue ?? key }),
}));

vi.mock('@renderer/components/chat/CollapsibleContent', () => ({
  default: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
}));
vi.mock('@renderer/components/Markdown', () => ({
  default: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
}));
vi.mock('@renderer/components/media/LocalImageView', () => ({ default: () => null }));
vi.mock('@/renderer/components/base/FileChangesPanel', () => ({ default: () => null }));
vi.mock('@/renderer/hooks/file/useDiffPreviewHandlers', () => ({
  useDiffPreviewHandlers: () => ({ handleFileClick: vi.fn(), handleDiffClick: vi.fn() }),
}));
vi.mock('@/renderer/hooks/context/FeedbackContext', () => ({
  useFeedback: () => ({ openFeedback: vi.fn() }),
}));
vi.mock('@/common', () => ({
  ipcBridge: {
    fs: { getImageBase64: { invoke: vi.fn() } },
    conversation: {
      confirmMessage: { invoke: vi.fn() },
      confirmation: { confirm: { invoke: vi.fn() } },
    },
  },
}));

afterEach(cleanup);

describe('browser capability UI', () => {
  it('renders a compact browser result card with wrapping URL content', () => {
    const message = {
      id: 'tool-group-1',
      conversation_id: 'conversation-1',
      type: 'tool_group',
      content: [
        {
          call_id: 'call-1',
          description: 'Browser navigation completed',
          name: 'browser_snapshot',
          render_output_as_markdown: false,
          status: 'Success',
          result_display: {
            kind: 'browser',
            action: 'snapshot',
            session_id: 'browser-session-1',
            title: '北京大学数学科学学院',
            url: 'https://www.math.pku.edu.cn/teachers/a-very-long-path-that-must-wrap-without-overflow',
            text: '教授名单页面内容',
            element_count: 18,
          },
        },
      ],
    } as IMessageToolGroup;

    render(<MessageToolGroup message={message} />);

    const card = screen.getByTestId('browser-tool-card');
    expect(card).toHaveClass('min-w-0', 'overflow-hidden');
    expect(screen.getByText('北京大学数学科学学院')).toBeInTheDocument();
    expect(screen.getByText(/a-very-long-path/)).toHaveClass('break-all');
    expect(screen.getByText(/18 interactive elements/)).toBeInTheDocument();
  });

  it('renders user takeover as a browser permission without an always-allow option', () => {
    const message = {
      id: 'permission-1',
      msg_id: 'message-1',
      conversation_id: 'conversation-1',
      type: 'permission',
      content: {
        id: 'approval-1',
        call_id: 'call-1',
        title: 'Continue browser task: browser_wait_for_user',
        action: 'browser',
        description: '请在可见浏览器中完成 OA 登录，然后继续',
        options: [
          { label: 'I have finished, continue', value: 'proceed' },
          { label: 'Cancel', value: 'cancel' },
        ],
      },
    } as IMessagePermission;

    render(<MessagePermission message={message} />);

    expect(screen.getByTestId('message-permission-card')).toHaveClass('min-w-0', 'overflow-hidden');
    expect(screen.getByText('🌐')).toBeInTheDocument();
    expect(screen.getByText('I have finished, continue')).toBeInTheDocument();
    expect(screen.queryByText('Allow always')).not.toBeInTheDocument();
  });

  it('renders browser metadata in the unified ACP tool summary', () => {
    const message = {
      id: 'browser-tool-message-1',
      conversation_id: 'conversation-1',
      type: 'acp_tool_call',
      content: {
        sessionId: 'conversation-1',
        update: {
          sessionUpdate: 'tool_call_update',
          tool_call_id: 'browser-tool-call-1',
          status: 'completed',
          title: 'browser_snapshot',
          kind: 'execute',
          raw_output: {
            output_summary: '{"kind":"browser","text":"truncated',
            browser: {
              kind: 'browser',
              action: 'snapshot',
              session_id: 'browser-session-1',
              title: '北京大学数学科学学院',
              url: 'https://www.math.pku.edu.cn/teachers/a-very-long-path-that-wraps',
              element_count: 18,
            },
          },
        },
      },
    } as IMessageAcpToolCall;

    render(<MessageToolGroupSummary messages={[message]} />);
    fireEvent.click(screen.getByText('View Steps · 1'));

    const card = screen.getByTestId('browser-tool-summary-card');
    expect(card).toHaveClass('browser-tool-summary');
    expect(screen.getByText('北京大学数学科学学院')).toBeInTheDocument();
    expect(screen.getByText(/a-very-long-path-that-wraps/)).toHaveClass('browser-tool-summary__url');
    expect(screen.getByText(/18 interactive elements/)).toBeInTheDocument();
  });
});
