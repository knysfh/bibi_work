import { describe, expect, it, vi } from 'vitest';
import { BrowserExecutionError, BrowserSessionManager } from '@process/browser/browserSessionManager';

function createFakeBrowser(type = 'text', scrollable = false) {
  let visibleText = 'Visible page text';
  let loginRequired = false;
  const fill = vi.fn(async () => undefined);
  const click = vi.fn(async () => undefined);
  const interactiveEvaluate = vi.fn(async () => 'input');
  const interactive = {
    isVisible: vi.fn(async () => true),
    evaluate: interactiveEvaluate,
    getAttribute: vi.fn(async (name: string) => {
      if (name === 'type') return type;
      if (name === 'placeholder') return 'Search';
      return null;
    }),
    innerText: vi.fn(async () => 'Interactive text'),
    click,
    fill,
  };
  const candidates = {
    count: vi.fn(async () => 1),
    nth: vi.fn(() => interactive),
    first: vi.fn(() => interactive),
  };
  const body = {
    innerText: vi.fn(async () => visibleText),
  };
  const goto = vi.fn(async () => undefined);
  const evaluate = vi.fn(async (_pageFunction: unknown, argument: unknown) =>
    typeof argument === 'object' && argument !== null && 'maxElements' in argument
      ? {
          title: 'Example',
          elements: [
            {
              ref: 'e1',
              tag: 'input',
              role: null,
              type,
              ariaLabel: 'Account',
              placeholder: 'Search',
              text: 'Interactive text',
              scrollable,
            },
          ],
          bodyText: visibleText,
          loginRequired,
        }
      : visibleText
  );
  const waitForTimeout = vi.fn(async () => undefined);
  const isClosed = vi.fn(() => false);
  const close = vi.fn(async () => isClosed.mockReturnValue(true));
  const url = vi.fn(() => 'https://example.com/');
  const title = vi.fn(async () => 'Example');
  const page: Record<string, unknown> = {
    bringToFront: vi.fn(async () => undefined),
    close,
    isClosed,
    goto,
    url,
    name: vi.fn(() => ''),
    title,
    locator: vi.fn((selector: string) =>
      selector === 'body'
        ? body
        : selector === '[ref="e16"] a'
          ? { count: vi.fn(async () => 0), first: vi.fn(() => interactive) }
          : candidates
    ),
    evaluate,
    waitForLoadState: vi.fn(async () => undefined),
    waitForTimeout,
    keyboard: { press: vi.fn(async () => undefined) },
  };
  page.frames = vi.fn(() => [page]);
  page.mainFrame = vi.fn(() => page);
  const pageHandlers: Array<(page: typeof page) => void> = [];
  const pages = vi.fn(() => [page]);
  const context = {
    pages,
    newPage: vi.fn(async () => page),
    close: vi.fn(async () => undefined),
    on: vi.fn((event: string, handler: (page: typeof page) => void) => {
      if (event === 'page') pageHandlers.push(handler);
    }),
  };
  return {
    context,
    click,
    close,
    evaluate,
    fill,
    goto,
    interactiveEvaluate,
    isClosed,
    page,
    pages,
    title,
    url,
    waitForTimeout,
    setLoginRequired: (value: boolean) => {
      loginRequired = value;
    },
    setVisibleText: (value: string) => {
      visibleText = value;
    },
    emitPage: (newPage: typeof page) => pageHandlers.forEach((handler) => handler(newPage)),
  };
}

describe('BrowserSessionManager', () => {
  it('rejects non-http URLs before launching a browser', async () => {
    const launch = vi.fn();
    const manager = new BrowserSessionManager({
      profilesDirectory: '/tmp/browser-profiles',
      launchPersistentContext: launch,
    });

    await expect(
      manager.execute({
        protocol: 'biwork_browser.v1',
        kind: 'browser',
        session_id: 'session-1',
        action: { name: 'open', url: 'file:///etc/passwd' },
      })
    ).rejects.toMatchObject({ code: 'BROWSER_URL_INVALID' });
    expect(launch).not.toHaveBeenCalled();
  });

  it('opens a persistent profile and returns a bounded semantic snapshot', async () => {
    const fake = createFakeBrowser();
    const launch = vi.fn(async () => fake.context as never);
    const manager = new BrowserSessionManager({
      profilesDirectory: '/tmp/browser-profiles',
      headless: true,
      launchPersistentContext: launch,
    });

    const result = await manager.execute({
      protocol: 'biwork_browser.v1',
      kind: 'browser',
      session_id: 'session-1',
      profile: 'oa',
      action: { name: 'open', url: 'https://example.com' },
    });

    expect(launch).toHaveBeenCalledWith('/tmp/browser-profiles/oa', expect.objectContaining({ headless: true }));
    expect(result).toMatchObject({
      kind: 'browser',
      action: 'open',
      session_id: 'session-1',
      profile: 'oa',
      title: 'Example',
      element_count: 1,
    });
  });

  it('accepts explicit HTTP URLs for intranet services', async () => {
    const fake = createFakeBrowser();
    const manager = new BrowserSessionManager({
      profilesDirectory: '/tmp/browser-profiles',
      headless: true,
      launchPersistentContext: vi.fn(async () => fake.context as never),
    });

    await manager.execute({
      protocol: 'biwork_browser.v1',
      kind: 'browser',
      session_id: 'session-http',
      action: { name: 'open', url: 'http://oa.internal.test/login' },
    });

    expect(fake.goto).toHaveBeenCalledWith(
      'http://oa.internal.test/login',
      expect.objectContaining({ waitUntil: 'domcontentloaded' })
    );
  });

  it('extracts text by snapshot ref and rejects a fabricated legacy selector immediately', async () => {
    const fake = createFakeBrowser();
    const manager = new BrowserSessionManager({
      profilesDirectory: '/tmp/browser-profiles',
      launchPersistentContext: vi.fn(async () => fake.context as never),
    });
    await manager.execute({
      protocol: 'biwork_browser.v1',
      kind: 'browser',
      session_id: 'session-1',
      action: { name: 'open', url: 'https://example.com' },
    });

    const extracted = await manager.execute({
      protocol: 'biwork_browser.v1',
      kind: 'browser',
      session_id: 'session-1',
      action: { name: 'extract_text', ref: 'e1' },
    });
    expect(extracted.text).toBe('Interactive text');

    await expect(
      manager.execute({
        protocol: 'biwork_browser.v1',
        kind: 'browser',
        session_id: 'session-1',
        action: { name: 'extract_text', selector: '[ref="e16"] a' },
      })
    ).rejects.toMatchObject({ code: 'BROWSER_TEXT_TARGET_NOT_FOUND' });
  });

  it('blocks model-driven password filling', async () => {
    const fake = createFakeBrowser('password');
    const manager = new BrowserSessionManager({
      profilesDirectory: '/tmp/browser-profiles',
      launchPersistentContext: vi.fn(async () => fake.context as never),
    });
    await manager.execute({
      protocol: 'biwork_browser.v1',
      kind: 'browser',
      session_id: 'session-1',
      action: { name: 'open', url: 'https://example.com' },
    });

    await expect(
      manager.execute({
        protocol: 'biwork_browser.v1',
        kind: 'browser',
        session_id: 'session-1',
        action: { name: 'fill', ref: 'e1', text: 'secret' },
      })
    ).rejects.toBeInstanceOf(BrowserExecutionError);
    await expect(
      manager.execute({
        protocol: 'biwork_browser.v1',
        kind: 'browser',
        session_id: 'session-1',
        action: { name: 'fill', ref: 'e1', text: 'secret' },
      })
    ).rejects.toMatchObject({ code: 'BROWSER_SENSITIVE_INPUT_BLOCKED' });
    expect(fake.fill).not.toHaveBeenCalled();
  });

  it('fails fast with a stable error when a snapshot ref is no longer actionable', async () => {
    const fake = createFakeBrowser();
    fake.click.mockRejectedValueOnce(new Error('outside of the viewport'));
    const manager = new BrowserSessionManager({
      profilesDirectory: '/tmp/browser-profiles',
      launchPersistentContext: vi.fn(async () => fake.context as never),
    });
    await manager.execute({
      protocol: 'biwork_browser.v1',
      kind: 'browser',
      session_id: 'session-1',
      action: { name: 'open', url: 'https://example.com' },
    });

    await expect(
      manager.execute({
        protocol: 'biwork_browser.v1',
        kind: 'browser',
        session_id: 'session-1',
        action: { name: 'click', ref: 'e1' },
      })
    ).rejects.toMatchObject({ code: 'BROWSER_TARGET_NOT_ACTIONABLE', retryable: true });
    expect(fake.click).toHaveBeenCalledWith({ timeout: 10_000 });
  });

  it('classifies a closed page during an element action as a recoverable lifecycle error', async () => {
    const fake = createFakeBrowser();
    fake.click.mockRejectedValueOnce(new Error('locator.click: Target page, context or browser has been closed'));
    const manager = new BrowserSessionManager({
      profilesDirectory: '/tmp/browser-profiles',
      launchPersistentContext: vi.fn(async () => fake.context as never),
    });
    await manager.execute({
      protocol: 'biwork_browser.v1',
      kind: 'browser',
      session_id: 'session-closed',
      action: { name: 'open', url: 'https://example.com' },
    });

    await expect(
      manager.execute({
        protocol: 'biwork_browser.v1',
        kind: 'browser',
        session_id: 'session-closed',
        action: { name: 'click', ref: 'e1' },
      })
    ).rejects.toMatchObject({ code: 'BROWSER_PAGE_NOT_FOUND', retryable: true });
  });

  it('classifies policy and protocol errors as non-recoverable', () => {
    expect(new BrowserExecutionError('BROWSER_REF_STALE', 'stale')).toMatchObject({ retryable: true });
    expect(new BrowserExecutionError('BROWSER_PAGE_NOT_FOUND', 'closed')).toMatchObject({ retryable: true });
    expect(new BrowserExecutionError('BROWSER_SESSION_NOT_FOUND', 'missing')).toMatchObject({ retryable: true });
    expect(new BrowserExecutionError('BROWSER_SENSITIVE_INPUT_BLOCKED', 'blocked')).toMatchObject({
      retryable: false,
    });
    expect(new BrowserExecutionError('BROWSER_URL_INVALID', 'invalid')).toMatchObject({ retryable: false });
  });

  it('retries a semantic snapshot when navigation destroys the page execution context', async () => {
    const fake = createFakeBrowser();
    fake.evaluate
      .mockResolvedValueOnce('settled')
      .mockRejectedValueOnce(new Error('Execution context was destroyed because of a navigation'));
    const manager = new BrowserSessionManager({
      profilesDirectory: '/tmp/browser-profiles',
      launchPersistentContext: vi.fn(async () => fake.context as never),
    });

    const result = await manager.execute({
      protocol: 'biwork_browser.v1',
      kind: 'browser',
      session_id: 'session-navigation',
      action: { name: 'open', url: 'https://example.com' },
    });

    expect(result).toMatchObject({ action: 'open', title: 'Example', tab_count: 1 });
    expect(fake.evaluate).toHaveBeenCalledTimes(3);
    expect(fake.waitForTimeout).toHaveBeenCalledWith(150);
  });

  it('reopens the last successful URL with the persistent profile after all pages are closed', async () => {
    const first = createFakeBrowser();
    const restored = createFakeBrowser();
    const launch = vi
      .fn()
      .mockResolvedValueOnce(first.context as never)
      .mockResolvedValueOnce(restored.context as never);
    const manager = new BrowserSessionManager({
      profilesDirectory: '/tmp/browser-profiles',
      headless: true,
      launchPersistentContext: launch,
    });
    await manager.execute({
      protocol: 'biwork_browser.v1',
      kind: 'browser',
      session_id: 'session-recover',
      profile: 'oa-test',
      action: { name: 'open', url: 'https://portal.example.test/attendance' },
    });
    first.isClosed.mockReturnValue(true);
    first.pages.mockReturnValue([]);

    const result = await manager.recoverSession('session-recover');

    expect(launch).toHaveBeenCalledTimes(2);
    expect(launch).toHaveBeenLastCalledWith(
      '/tmp/browser-profiles/oa-test',
      expect.objectContaining({ headless: true })
    );
    expect(restored.goto).toHaveBeenCalledWith(
      'https://example.com/',
      expect.objectContaining({ waitUntil: 'domcontentloaded' })
    );
    expect(result).toMatchObject({ action: 'recover', session_id: 'session-recover', title: 'Example' });
  });

  it('reclaims a persistent profile from a closed stale session when the agent opens a new session', async () => {
    const first = createFakeBrowser();
    const reopened = createFakeBrowser();
    const launch = vi
      .fn()
      .mockResolvedValueOnce(first.context as never)
      .mockResolvedValueOnce(reopened.context as never);
    const manager = new BrowserSessionManager({
      profilesDirectory: '/tmp/browser-profiles',
      launchPersistentContext: launch,
    });
    await manager.execute({
      protocol: 'biwork_browser.v1',
      kind: 'browser',
      session_id: 'old-session',
      profile: 'shared-profile',
      action: { name: 'open', url: 'https://portal.example.test/attendance' },
    });
    first.isClosed.mockReturnValue(true);
    first.pages.mockReturnValue([]);

    const result = await manager.execute({
      protocol: 'biwork_browser.v1',
      kind: 'browser',
      session_id: 'new-session',
      profile: 'shared-profile',
      action: { name: 'open', url: 'https://portal.example.test/work-hours' },
    });

    expect(first.context.close).toHaveBeenCalled();
    expect(launch).toHaveBeenCalledTimes(2);
    expect(result).toMatchObject({ action: 'open', session_id: 'new-session', profile: 'shared-profile' });
  });

  it('lists, opens, selects, and closes tabs by stable tab id', async () => {
    const first = createFakeBrowser();
    const second = createFakeBrowser();
    second.url.mockReturnValue('https://example.com/reports');
    second.title.mockResolvedValue('Reports');
    const openPages = [first.page];
    first.pages.mockImplementation(() => openPages);
    first.context.newPage.mockImplementation(async () => {
      openPages.push(second.page);
      return second.page;
    });
    const manager = new BrowserSessionManager({
      profilesDirectory: '/tmp/browser-profiles',
      launchPersistentContext: vi.fn(async () => first.context as never),
    });

    const opened = await manager.execute({
      protocol: 'biwork_browser.v1',
      kind: 'browser',
      session_id: 'tabs-session',
      action: { name: 'open', url: 'https://example.com' },
    });
    const newTab = await manager.execute({
      protocol: 'biwork_browser.v1',
      kind: 'browser',
      session_id: 'tabs-session',
      action: { name: 'tab_open', url: 'https://example.com/reports' },
    });
    const listed = await manager.execute({
      protocol: 'biwork_browser.v1',
      kind: 'browser',
      session_id: 'tabs-session',
      action: { name: 'tab_list' },
    });

    expect(opened).toMatchObject({ tab_id: 't1', tab_count: 1 });
    expect(newTab).toMatchObject({ tab_id: 't2', tab_count: 2, title: 'Reports' });
    expect(listed.tabs).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ tab_id: 't1', active: false }),
        expect.objectContaining({ tab_id: 't2', active: true }),
      ])
    );

    const selected = await manager.execute({
      protocol: 'biwork_browser.v1',
      kind: 'browser',
      session_id: 'tabs-session',
      action: { name: 'tab_select', tab_id: 't1' },
    });
    const closed = await manager.execute({
      protocol: 'biwork_browser.v1',
      kind: 'browser',
      session_id: 'tabs-session',
      action: { name: 'tab_close', tab_id: 't2' },
    });

    expect(selected).toMatchObject({ action: 'tab_select', tab_id: 't1' });
    expect(closed).toMatchObject({ action: 'tab_close', tab_id: 't1', tab_count: 1 });
    expect(second.close).toHaveBeenCalledOnce();
  });

  it('marks a return to a visible password page as an expired login state', async () => {
    const fake = createFakeBrowser();
    const manager = new BrowserSessionManager({
      profilesDirectory: '/tmp/browser-profiles',
      launchPersistentContext: vi.fn(async () => fake.context as never),
    });
    await manager.execute({
      protocol: 'biwork_browser.v1',
      kind: 'browser',
      session_id: 'auth-session',
      action: { name: 'open', url: 'https://example.com/dashboard' },
    });
    fake.setLoginRequired(true);

    const expired = await manager.execute({
      protocol: 'biwork_browser.v1',
      kind: 'browser',
      session_id: 'auth-session',
      action: { name: 'snapshot' },
    });

    expect(expired).toMatchObject({ auth_state: 'login_required', auth_expired: true });
  });

  it('waits for SPA content to change before returning a fresh snapshot', async () => {
    const fake = createFakeBrowser();
    const manager = new BrowserSessionManager({
      profilesDirectory: '/tmp/browser-profiles',
      launchPersistentContext: vi.fn(async () => fake.context as never),
    });
    await manager.execute({
      protocol: 'biwork_browser.v1',
      kind: 'browser',
      session_id: 'spa-session',
      action: { name: 'open', url: 'https://example.com/app' },
    });
    fake.waitForTimeout.mockImplementationOnce(async () => fake.setVisibleText('SPA report loaded'));

    const changed = await manager.execute({
      protocol: 'biwork_browser.v1',
      kind: 'browser',
      session_id: 'spa-session',
      action: { name: 'wait_for_change', timeout_ms: 1_000 },
    });

    expect(changed).toMatchObject({ action: 'wait_for_change', text: 'SPA report loaded' });
  });

  it('scrolls a snapshot-marked virtual list container and refreshes refs', async () => {
    const fake = createFakeBrowser('text', true);
    const manager = new BrowserSessionManager({
      profilesDirectory: '/tmp/browser-profiles',
      launchPersistentContext: vi.fn(async () => fake.context as never),
    });
    await manager.execute({
      protocol: 'biwork_browser.v1',
      kind: 'browser',
      session_id: 'virtual-list-session',
      action: { name: 'open', url: 'https://example.com/work-hours' },
    });

    const scrolled = await manager.execute({
      protocol: 'biwork_browser.v1',
      kind: 'browser',
      session_id: 'virtual-list-session',
      action: { name: 'scroll', ref: 'e1', delta_y: 900 },
    });

    expect(fake.interactiveEvaluate).toHaveBeenCalledWith(expect.any(Function), { x: 0, y: 900 });
    expect(scrolled).toMatchObject({ action: 'scroll', elements: [expect.objectContaining({ scrollable: true })] });
  });
});
