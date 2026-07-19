import { describe, expect, it, vi } from 'vitest';
import { BrowserExecutionError, type BrowserSessionManager } from '@process/browser/browserSessionManager';
import { resolveBrowserWorkItem, type BrowserWorkItem } from '@process/browser/browserWorker';

function workItem(action: BrowserWorkItem['command']['action']): BrowserWorkItem {
  return {
    id: 'request-1',
    tenant_id: 'tenant-1',
    command: {
      protocol: 'biwork_browser.v1',
      kind: 'browser',
      session_id: 'session-1',
      profile: 'default',
      action,
    },
  };
}

describe('resolveBrowserWorkItem', () => {
  it('returns a recoverable failed result with a refreshed snapshot', async () => {
    const execute = vi
      .fn()
      .mockRejectedValueOnce(
        new BrowserExecutionError('BROWSER_TARGET_NOT_ACTIONABLE', 'Browser element e3 is no longer actionable')
      )
      .mockResolvedValueOnce({
        kind: 'browser',
        action: 'snapshot',
        session_id: 'session-1',
        url: 'https://portal.example.test/home',
        elements: [{ ref: 'e7', label: 'Attendance' }],
        element_count: 1,
      });
    const manager = { execute } as unknown as BrowserSessionManager;

    const completion = await resolveBrowserWorkItem(manager, workItem({ name: 'click', ref: 'e3' }));

    expect(completion.status).toBe('failed');
    expect(completion.error).toContain('BROWSER_TARGET_NOT_ACTIONABLE');
    expect(completion.result).toMatchObject({
      kind: 'browser',
      action: 'click',
      session_id: 'session-1',
      status: 'failed',
      retryable: true,
      error: { code: 'BROWSER_TARGET_NOT_ACTIONABLE' },
      recovery_snapshot: {
        action: 'snapshot',
        elements: [{ ref: 'e7', label: 'Attendance' }],
      },
    });
    expect(execute).toHaveBeenNthCalledWith(2, expect.objectContaining({ action: { name: 'snapshot' } }));
  });

  it('keeps non-recoverable browser failures terminal', async () => {
    const execute = vi.fn().mockRejectedValue(new BrowserExecutionError('BROWSER_SENSITIVE_INPUT_BLOCKED', 'blocked'));
    const manager = { execute } as unknown as BrowserSessionManager;

    const completion = await resolveBrowserWorkItem(manager, workItem({ name: 'fill', ref: 'e2', text: 'x' }));

    expect(completion).toMatchObject({ status: 'failed', result: null });
    expect(execute).toHaveBeenCalledOnce();
  });

  it('restores a manually closed page and returns the fresh snapshot to the agent', async () => {
    const execute = vi
      .fn()
      .mockRejectedValueOnce(new BrowserExecutionError('BROWSER_TARGET_NOT_ACTIONABLE', 'Target page has been closed'))
      .mockRejectedValueOnce(new BrowserExecutionError('BROWSER_PAGE_NOT_FOUND', 'No open pages'));
    const recoverSession = vi.fn().mockResolvedValue({
      kind: 'browser',
      action: 'recover',
      session_id: 'session-1',
      url: 'https://portal.example.test/attendance',
      elements: [{ ref: 'e4', label: 'Work hours' }],
      element_count: 1,
    });
    const manager = { execute, recoverSession } as unknown as BrowserSessionManager;

    const completion = await resolveBrowserWorkItem(manager, workItem({ name: 'click', ref: 'e3' }));

    expect(recoverSession).toHaveBeenCalledWith('session-1');
    expect(completion.result).toMatchObject({
      retryable: true,
      recovery_action: 'page_restored',
      recovery_snapshot: {
        action: 'recover',
        elements: [{ ref: 'e4', label: 'Work hours' }],
      },
    });
  });

  it('asks the agent to rebuild a browser environment when the old session no longer exists', async () => {
    const execute = vi
      .fn()
      .mockRejectedValue(new BrowserExecutionError('BROWSER_SESSION_NOT_FOUND', 'Session missing'));
    const manager = { execute, recoverSession: vi.fn() } as unknown as BrowserSessionManager;

    const completion = await resolveBrowserWorkItem(manager, workItem({ name: 'snapshot' }));

    expect(completion.result).toMatchObject({
      retryable: true,
      recovery_action: 'browser_open_required',
      recovery_snapshot: null,
    });
  });

  it('completes a snapshot transparently when restoring the closed page fulfills the request', async () => {
    const execute = vi.fn().mockRejectedValue(new BrowserExecutionError('BROWSER_PAGE_NOT_FOUND', 'No open pages'));
    const recoverSession = vi.fn().mockResolvedValue({
      kind: 'browser',
      action: 'recover',
      session_id: 'session-1',
      url: 'https://portal.example.test/attendance',
      title: 'Attendance',
    });
    const manager = { execute, recoverSession } as unknown as BrowserSessionManager;

    const completion = await resolveBrowserWorkItem(manager, workItem({ name: 'snapshot' }));

    expect(completion).toMatchObject({
      status: 'completed',
      error: null,
      result: {
        action: 'recover',
        recovered: true,
        recovery_action: 'page_restored',
      },
    });
  });
});
