import fs from 'node:fs';
import path from 'node:path';
import type { BrowserContext, Frame, Locator, Page } from 'playwright';
import { chromium } from 'playwright';

const MAX_INTERACTIVE_ELEMENTS = 120;
const MAX_TEXT_LENGTH = 20_000;
const NAVIGATION_TIMEOUT_MS = 60_000;
const ELEMENT_ACTION_TIMEOUT_MS = 10_000;
const TEXT_EXTRACTION_TIMEOUT_MS = 3_000;
const PAGE_STABILITY_TIMEOUT_MS = 10_000;
const PAGE_STABILITY_RETRY_DELAY_MS = 150;
const SPA_QUIET_PERIOD_MS = 300;
const SPA_SETTLE_TIMEOUT_MS = 3_000;
const DEFAULT_CHANGE_TIMEOUT_MS = 10_000;
const MAX_CHANGE_TIMEOUT_MS = 30_000;
const MAX_SCROLL_DELTA = 5_000;
const BIWORK_REF_ATTRIBUTE = 'data-biwork-browser-ref';
const INTERACTIVE_ELEMENT_SELECTOR =
  'a,button,input,textarea,select,[role="button"],[role="link"],[contenteditable="true"],[onclick],[tabindex]:not([tabindex="-1"])';

export const RECOVERABLE_BROWSER_ERROR_CODES: ReadonlySet<string> = new Set([
  'BROWSER_REF_STALE',
  'BROWSER_TARGET_NOT_ACTIONABLE',
  'BROWSER_PAGE_UNSTABLE',
  'BROWSER_PAGE_UNCHANGED',
  'BROWSER_PAGE_NOT_FOUND',
  'BROWSER_SESSION_NOT_FOUND',
  'BROWSER_TAB_NOT_FOUND',
  'BROWSER_TEXT_TARGET_NOT_FOUND',
  'BROWSER_TEXT_TARGET_UNAVAILABLE',
]);

export function isRecoverableBrowserErrorCode(code: string): boolean {
  return RECOVERABLE_BROWSER_ERROR_CODES.has(code);
}

export class BrowserExecutionError extends Error {
  public readonly retryable: boolean;

  constructor(
    public readonly code: string,
    message: string
  ) {
    super(message);
    this.name = 'BrowserExecutionError';
    this.retryable = isRecoverableBrowserErrorCode(code);
  }
}

export type BrowserCommand = {
  protocol?: unknown;
  kind?: unknown;
  session_id?: unknown;
  profile?: unknown;
  action?: {
    name?: unknown;
    url?: unknown;
    ref?: unknown;
    text?: unknown;
    key?: unknown;
    selector?: unknown;
    reason?: unknown;
    expected_url?: unknown;
    tab_id?: unknown;
    timeout_ms?: unknown;
    delta_x?: unknown;
    delta_y?: unknown;
  };
};

type BrowserSession = {
  context: BrowserContext;
  page: Page;
  profile: string;
  refs: Map<string, Locator>;
  lastUrl: string;
  tabIds: WeakMap<Page, string>;
  nextTabNumber: number;
  lastPageSignal: string | null;
  authenticatedObserved: boolean;
};

type FrameSnapshotElement = {
  ref: string;
  tag: string;
  role: string | null;
  type: string | null;
  ariaLabel: string | null;
  placeholder: string | null;
  text: string;
  scrollable: boolean;
};

type FrameSnapshot = {
  id: string;
  url: string;
  name: string | null;
  title: string;
  text: string;
  elements: FrameSnapshotElement[];
  loginRequired: boolean;
};

type PersistentContextLauncher = (
  userDataDir: string,
  options: Parameters<typeof chromium.launchPersistentContext>[1]
) => Promise<BrowserContext>;

export type BrowserSessionManagerOptions = {
  profilesDirectory: string;
  headless?: boolean;
  launchPersistentContext?: PersistentContextLauncher;
};

export class BrowserSessionManager {
  private readonly sessions = new Map<string, BrowserSession>();
  private readonly profileLeases = new Map<string, string>();
  private readonly launchPersistentContext: PersistentContextLauncher;

  constructor(private readonly options: BrowserSessionManagerOptions) {
    this.launchPersistentContext =
      options.launchPersistentContext ??
      ((userDataDir, launchOptions) => chromium.launchPersistentContext(userDataDir, launchOptions));
  }

  async execute(command: BrowserCommand): Promise<Record<string, unknown>> {
    if (command.protocol !== 'biwork_browser.v1' || command.kind !== 'browser') {
      throw new BrowserExecutionError('BROWSER_WORK_INVALID', 'Unsupported browser work item');
    }
    const sessionId = requiredString(command.session_id, 'session_id');
    const action = command.action;
    const actionName = requiredString(action?.name, 'action.name');

    try {
      switch (actionName) {
        case 'open':
          return this.open(sessionId, optionalProfile(command.profile), requiredHttpUrl(action?.url));
        case 'goto':
          return this.goto(sessionId, requiredHttpUrl(action?.url));
        case 'snapshot':
          return this.snapshotResult(sessionId, actionName);
        case 'tab_list':
          return this.tabList(sessionId);
        case 'tab_open':
          return this.tabOpen(sessionId, requiredHttpUrl(action?.url));
        case 'tab_select':
          return this.tabSelect(sessionId, requiredString(action?.tab_id, 'action.tab_id'));
        case 'tab_close':
          return this.tabClose(sessionId, requiredString(action?.tab_id, 'action.tab_id'));
        case 'click':
          return this.click(sessionId, requiredString(action?.ref, 'action.ref'));
        case 'fill':
          return this.fill(
            sessionId,
            requiredString(action?.ref, 'action.ref'),
            requiredString(action?.text, 'action.text', true)
          );
        case 'press':
          return this.press(sessionId, requiredString(action?.key, 'action.key'));
        case 'scroll':
          return this.scroll(
            sessionId,
            optionalString(action?.ref),
            optionalBoundedInteger(action?.delta_x, 'action.delta_x', -MAX_SCROLL_DELTA, MAX_SCROLL_DELTA, 0),
            optionalBoundedInteger(action?.delta_y, 'action.delta_y', -MAX_SCROLL_DELTA, MAX_SCROLL_DELTA, 700)
          );
        case 'wait_for_change':
          return this.waitForChange(
            sessionId,
            optionalBoundedInteger(
              action?.timeout_ms,
              'action.timeout_ms',
              1_000,
              MAX_CHANGE_TIMEOUT_MS,
              DEFAULT_CHANGE_TIMEOUT_MS
            )
          );
        case 'extract_text':
          return this.extractText(sessionId, optionalString(action?.ref), optionalString(action?.selector));
        case 'wait_for_user':
          return this.waitForUser(sessionId, optionalString(action?.expected_url));
        case 'close':
          return this.close(sessionId);
        default:
          throw new BrowserExecutionError('BROWSER_ACTION_UNSUPPORTED', `Unsupported browser action: ${actionName}`);
      }
    } catch (error) {
      if (error instanceof BrowserExecutionError) throw error;
      if (isClosedBrowserStateError(error)) {
        throw new BrowserExecutionError(
          'BROWSER_PAGE_NOT_FOUND',
          'The previous browser page or context was closed. Restore the page and continue with a fresh snapshot.'
        );
      }
      throw error;
    }
  }

  async closeAll(): Promise<void> {
    const sessions = [...this.sessions.entries()];
    this.sessions.clear();
    this.profileLeases.clear();
    await Promise.allSettled(sessions.map(([, session]) => session.context.close()));
  }

  async recoverSession(sessionId: string): Promise<Record<string, unknown>> {
    const session = this.requiredSession(sessionId);
    return this.relaunchSession(sessionId, session, session.lastUrl, 'recover');
  }

  private async open(sessionId: string, profile: string, url: string): Promise<Record<string, unknown>> {
    const existing = this.sessions.get(sessionId);
    if (existing) {
      const page = lastOpenPage(existing.context);
      if (!page) return this.relaunchSession(sessionId, existing, url, 'open');
      existing.page = page;
      await page.bringToFront();
      await page.goto(url, { waitUntil: 'domcontentloaded', timeout: NAVIGATION_TIMEOUT_MS });
      await this.waitForSpaSettled(page);
      return this.snapshotResult(sessionId, 'open');
    }
    const leasedBy = this.profileLeases.get(profile);
    if (leasedBy) {
      const leasedSession = this.sessions.get(leasedBy);
      if (!leasedSession || !lastOpenPage(leasedSession.context)) {
        if (leasedSession) await this.discardSession(leasedBy, leasedSession);
        else this.profileLeases.delete(profile);
      } else {
        throw new BrowserExecutionError(
          'BROWSER_PROFILE_BUSY',
          `Browser profile ${profile} is already used by session ${leasedBy}`
        );
      }
    }

    const context = await this.launchPersistentContext(path.join(this.options.profilesDirectory, profile), {
      headless: this.options.headless ?? process.env.BIWORK_BROWSER_HEADLESS === '1',
      executablePath: resolveBrowserExecutablePath(),
      viewport: null,
      acceptDownloads: true,
    });
    const page = context.pages()[0] ?? (await context.newPage());
    const session: BrowserSession = {
      context,
      page,
      profile,
      refs: new Map(),
      lastUrl: url,
      tabIds: new WeakMap(),
      nextTabNumber: 1,
      lastPageSignal: null,
      authenticatedObserved: false,
    };
    this.sessions.set(sessionId, session);
    this.observePages(session);
    this.profileLeases.set(profile, sessionId);
    try {
      await page.bringToFront();
      await page.goto(url, { waitUntil: 'domcontentloaded', timeout: NAVIGATION_TIMEOUT_MS });
      await this.waitForSpaSettled(page);
      return await this.snapshotResult(sessionId, 'open');
    } catch (error) {
      await this.close(sessionId);
      throw error;
    }
  }

  private async relaunchSession(
    sessionId: string,
    session: BrowserSession,
    url: string,
    action: string
  ): Promise<Record<string, unknown>> {
    await session.context.close().catch((): void => undefined);
    const context = await this.launchPersistentContext(path.join(this.options.profilesDirectory, session.profile), {
      headless: this.options.headless ?? process.env.BIWORK_BROWSER_HEADLESS === '1',
      executablePath: resolveBrowserExecutablePath(),
      viewport: null,
      acceptDownloads: true,
    });
    const page = context.pages()[0] ?? (await context.newPage());
    session.context = context;
    session.page = page;
    session.refs.clear();
    session.lastUrl = url;
    session.tabIds = new WeakMap();
    session.nextTabNumber = 1;
    session.lastPageSignal = null;
    this.observePages(session);
    this.profileLeases.set(session.profile, sessionId);
    try {
      await page.bringToFront();
      await page.goto(url, { waitUntil: 'domcontentloaded', timeout: NAVIGATION_TIMEOUT_MS });
      await this.waitForSpaSettled(page);
      return await this.snapshotResult(sessionId, action);
    } catch (error) {
      await this.discardSession(sessionId, session);
      throw error;
    }
  }

  private observePages(session: BrowserSession): void {
    for (const page of session.context.pages()) this.tabId(session, page);
    session.context.on('page', (newPage) => {
      this.tabId(session, newPage);
      session.page = newPage;
      session.refs.clear();
      void newPage.bringToFront().catch((): void => undefined);
    });
  }

  private async discardSession(sessionId: string, session: BrowserSession): Promise<void> {
    this.sessions.delete(sessionId);
    if (this.profileLeases.get(session.profile) === sessionId) this.profileLeases.delete(session.profile);
    await session.context.close().catch((): void => undefined);
  }

  private async goto(sessionId: string, url: string): Promise<Record<string, unknown>> {
    const session = this.requiredSession(sessionId);
    const page = this.activePage(session);
    await page.goto(url, { waitUntil: 'domcontentloaded', timeout: NAVIGATION_TIMEOUT_MS });
    await this.waitForSpaSettled(page);
    return this.snapshotResult(sessionId, 'goto');
  }

  private async tabList(sessionId: string): Promise<Record<string, unknown>> {
    const session = this.requiredSession(sessionId);
    const pages = this.openPages(session);
    return {
      kind: 'browser',
      action: 'tab_list',
      session_id: sessionId,
      tab_count: pages.length,
      tabs: await this.tabMetadata(session, pages),
    };
  }

  private async tabOpen(sessionId: string, url: string): Promise<Record<string, unknown>> {
    const session = this.requiredSession(sessionId);
    const page = await session.context.newPage();
    this.tabId(session, page);
    session.page = page;
    session.refs.clear();
    await page.bringToFront();
    await page.goto(url, { waitUntil: 'domcontentloaded', timeout: NAVIGATION_TIMEOUT_MS });
    await this.waitForSpaSettled(page);
    return this.snapshotResult(sessionId, 'tab_open');
  }

  private async tabSelect(sessionId: string, tabId: string): Promise<Record<string, unknown>> {
    const session = this.requiredSession(sessionId);
    const page = this.requiredTab(session, tabId);
    session.page = page;
    session.refs.clear();
    await page.bringToFront();
    return this.snapshotResult(sessionId, 'tab_select');
  }

  private async tabClose(sessionId: string, tabId: string): Promise<Record<string, unknown>> {
    const session = this.requiredSession(sessionId);
    const page = this.requiredTab(session, tabId);
    const wasActive = page === session.page;
    await page.close();
    session.refs.clear();
    const pages = this.openPages(session);
    if (pages.length === 0) {
      return {
        kind: 'browser',
        action: 'tab_close',
        session_id: sessionId,
        closed_tab_id: tabId,
        tab_count: 0,
        tabs: [],
      };
    }
    if (wasActive || session.page.isClosed()) session.page = pages.at(-1)!;
    await session.page.bringToFront();
    return this.snapshotResult(sessionId, 'tab_close');
  }

  private async click(sessionId: string, ref: string): Promise<Record<string, unknown>> {
    const session = this.requiredSession(sessionId);
    const locator = this.requiredRef(session, ref);
    try {
      await locator.click({ timeout: ELEMENT_ACTION_TIMEOUT_MS });
    } catch (error) {
      const cause = compactErrorCause(error);
      if (isClosedBrowserStateError(error)) {
        throw new BrowserExecutionError(
          'BROWSER_PAGE_NOT_FOUND',
          `The previous browser page or context was closed.${cause ? ` Playwright reported: ${cause}` : ''}`
        );
      }
      throw new BrowserExecutionError(
        'BROWSER_TARGET_NOT_ACTIONABLE',
        `Browser element ${ref} is no longer visible or actionable. Take a new snapshot and retry with a visible ref.${cause ? ` Playwright reported: ${cause}` : ''}`
      );
    }
    await this.activePage(session)
      .waitForLoadState('domcontentloaded', { timeout: 5_000 })
      .catch((): void => undefined);
    await this.waitForSpaSettled(this.activePage(session));
    return this.snapshotResult(sessionId, 'click');
  }

  private async fill(sessionId: string, ref: string, text: string): Promise<Record<string, unknown>> {
    const session = this.requiredSession(sessionId);
    const locator = this.requiredRef(session, ref);
    const inputType = (await locator.getAttribute('type'))?.toLowerCase();
    if (inputType === 'password') {
      throw new BrowserExecutionError(
        'BROWSER_SENSITIVE_INPUT_BLOCKED',
        'Password fields must be completed by the user in the visible browser'
      );
    }
    await locator.fill(text, { timeout: NAVIGATION_TIMEOUT_MS });
    await this.waitForSpaSettled(this.activePage(session));
    return this.snapshotResult(sessionId, 'fill');
  }

  private async press(sessionId: string, key: string): Promise<Record<string, unknown>> {
    const session = this.requiredSession(sessionId);
    await this.activePage(session).keyboard.press(key);
    await this.activePage(session)
      .waitForLoadState('domcontentloaded', { timeout: 5_000 })
      .catch((): void => undefined);
    await this.waitForSpaSettled(this.activePage(session));
    return this.snapshotResult(sessionId, 'press');
  }

  private async scroll(
    sessionId: string,
    ref: string | null,
    deltaX: number,
    deltaY: number
  ): Promise<Record<string, unknown>> {
    const session = this.requiredSession(sessionId);
    const page = this.activePage(session);
    if (ref) {
      const locator = this.requiredRef(session, ref);
      await locator.evaluate(
        (element, delta) => {
          element.scrollBy({ left: delta.x, top: delta.y, behavior: 'instant' });
        },
        { x: deltaX, y: deltaY }
      );
    } else {
      await page.evaluate((delta) => window.scrollBy({ left: delta.x, top: delta.y, behavior: 'instant' }), {
        x: deltaX,
        y: deltaY,
      });
    }
    await this.waitForSpaSettled(page);
    return this.snapshotResult(sessionId, 'scroll');
  }

  private async waitForChange(sessionId: string, timeoutMs: number): Promise<Record<string, unknown>> {
    const session = this.requiredSession(sessionId);
    const baseline = session.lastPageSignal;
    if (!baseline) {
      await this.waitForSpaSettled(this.activePage(session));
      return this.snapshotResult(sessionId, 'wait_for_change');
    }
    const deadline = Date.now() + timeoutMs;
    while (Date.now() < deadline) {
      const page = this.activePage(session);
      // eslint-disable-next-line no-await-in-loop
      const signal = await this.currentPageSignal(page);
      if (signal !== baseline) {
        // eslint-disable-next-line no-await-in-loop
        await this.waitForSpaSettled(page);
        return this.snapshotResult(sessionId, 'wait_for_change');
      }
      // eslint-disable-next-line no-await-in-loop
      await page.waitForTimeout(PAGE_STABILITY_RETRY_DELAY_MS);
    }
    throw new BrowserExecutionError(
      'BROWSER_PAGE_UNCHANGED',
      `The page did not change within ${timeoutMs}ms. Re-check the target, scroll the relevant container, or choose another action.`
    );
  }

  private async extractText(
    sessionId: string,
    ref: string | null,
    legacySelector: string | null
  ): Promise<Record<string, unknown>> {
    const session = this.requiredSession(sessionId);
    let text: string;
    if (ref) {
      try {
        text = await this.requiredRef(session, ref).innerText({ timeout: TEXT_EXTRACTION_TIMEOUT_MS });
      } catch {
        throw new BrowserExecutionError(
          'BROWSER_REF_STALE',
          `Browser element reference is missing or stale: ${ref}. Take a new snapshot and retry.`
        );
      }
    } else if (legacySelector) {
      const matches = this.activePage(session).locator(legacySelector);
      if ((await matches.count()) === 0) {
        throw new BrowserExecutionError(
          'BROWSER_TEXT_TARGET_NOT_FOUND',
          'The legacy CSS selector did not match an element. Use a ref from the latest browser snapshot.'
        );
      }
      try {
        text = await matches.first().innerText({ timeout: TEXT_EXTRACTION_TIMEOUT_MS });
      } catch {
        throw new BrowserExecutionError(
          'BROWSER_TEXT_TARGET_UNAVAILABLE',
          'The selected browser element was not readable. Take a new snapshot and use its ref.'
        );
      }
    } else {
      const { value } = await this.withStablePage(session, (stablePage) => this.collectFrameText(stablePage));
      return {
        kind: 'browser',
        action: 'extract_text',
        session_id: sessionId,
        url: this.activePage(session).url(),
        title: await this.activePage(session).title(),
        text: value.text,
        frames: value.frames,
        frame_count: value.frames.length,
        truncated: value.truncated,
      };
    }
    text = text.slice(0, MAX_TEXT_LENGTH);
    return {
      kind: 'browser',
      action: 'extract_text',
      session_id: sessionId,
      url: this.activePage(session).url(),
      title: await this.activePage(session).title(),
      text,
      truncated: text.length >= MAX_TEXT_LENGTH,
    };
  }

  private async waitForUser(sessionId: string, expectedUrl: string | null): Promise<Record<string, unknown>> {
    const session = this.requiredSession(sessionId);
    const latestPage = lastOpenPage(session.context);
    if (latestPage) {
      session.page = latestPage;
      session.refs.clear();
    }
    const page = this.activePage(session);
    await page.bringToFront();
    if (expectedUrl && !page.url().includes(expectedUrl)) {
      throw new BrowserExecutionError(
        'BROWSER_USER_ACTION_INCOMPLETE',
        `Browser is not at the expected URL after user takeover: ${expectedUrl}`
      );
    }
    return this.snapshotResult(sessionId, 'wait_for_user');
  }

  private async close(sessionId: string): Promise<Record<string, unknown>> {
    const session = this.requiredSession(sessionId);
    this.sessions.delete(sessionId);
    this.profileLeases.delete(session.profile);
    await session.context.close();
    return { kind: 'browser', action: 'close', session_id: sessionId, closed: true };
  }

  private async snapshotResult(sessionId: string, action: string): Promise<Record<string, unknown>> {
    const session = this.requiredSession(sessionId);
    const { value } = await this.withStablePage(session, async (stablePage) => {
      const snapshot = await this.collectFrameSnapshot(stablePage);
      return {
        snapshot,
        title: await stablePage.title(),
        url: stablePage.url(),
      };
    });
    const { snapshot, title, url } = value;
    if (/^https?:\/\//i.test(url)) session.lastUrl = url;
    const loginRequired = snapshot.frames.some((frame) => frame.loginRequired);
    const authExpired = loginRequired && session.authenticatedObserved;
    if (!loginRequired) session.authenticatedObserved = true;
    session.lastPageSignal = pageSignal(url, title, snapshot.frames[0]?.text ?? snapshot.text);
    session.refs.clear();
    const frames = snapshot.frames.map((frame) => ({
      id: frame.id,
      url: frame.url,
      name: frame.name,
      title: frame.title,
      text: frame.text,
      element_count: frame.elements.length,
      login_required: frame.loginRequired,
    }));
    const elements = snapshot.frames.flatMap((frame) =>
      frame.elements.map((element) => {
        session.refs.set(element.ref, frame.frame.locator(`[${BIWORK_REF_ATTRIBUTE}="${element.ref}"]`).first());
        return {
          ref: element.ref,
          tag: element.tag,
          role: element.role,
          type: element.type,
          scrollable: element.scrollable,
          label: compactText(element.ariaLabel || element.placeholder || element.text, 240),
          frame_id: frame.id,
          frame_url: frame.url,
        };
      })
    );
    const pages = this.openPages(session);
    return {
      kind: 'browser',
      action,
      session_id: sessionId,
      profile: session.profile,
      url,
      title,
      auth_state: loginRequired ? 'login_required' : 'authenticated_or_public',
      auth_expired: authExpired,
      text: snapshot.text,
      elements,
      element_count: elements.length,
      frames,
      frame_count: frames.length,
      tab_id: this.tabId(session, session.page),
      tab_count: pages.length,
      tabs: await this.tabMetadata(session, pages),
    };
  }

  private async collectFrameSnapshot(page: Page): Promise<{
    frames: Array<FrameSnapshot & { frame: Frame }>;
    text: string;
  }> {
    const frames: Array<FrameSnapshot & { frame: Frame }> = [];
    let remainingElements = MAX_INTERACTIVE_ELEMENTS;
    let nextRefNumber = 1;
    for (const [frameIndex, frame] of page.frames().entries()) {
      // Frame snapshots are intentionally sequential so refs stay globally unique under the shared element cap.
      // eslint-disable-next-line no-await-in-loop
      if (!(await this.isVisibleFrame(page, frame))) continue;
      const frameId = `f${frameIndex}`;
      // eslint-disable-next-line no-await-in-loop
      const snapshot = await frame.evaluate(
        ({ interactiveSelector, maxElements, maxTextLength, refAttribute, firstRefNumber }) => {
          document.querySelectorAll(`[${refAttribute}]`).forEach((element) => element.removeAttribute(refAttribute));
          const candidates = Array.from(document.querySelectorAll<HTMLElement>('*'));
          const elements: FrameSnapshotElement[] = [];
          for (const element of candidates) {
            if (elements.length >= maxElements) break;
            const style = window.getComputedStyle(element);
            const labelText = (
              element.getAttribute('aria-label') ||
              element.getAttribute('title') ||
              element.innerText ||
              element.textContent ||
              ''
            ).trim();
            const parentCursor = element.parentElement ? window.getComputedStyle(element.parentElement).cursor : null;
            const isPointerRoot = style.cursor === 'pointer' && parentCursor !== 'pointer' && Boolean(labelText);
            const rect = element.getBoundingClientRect();
            const isScrollable =
              Boolean(labelText) &&
              ((element.scrollHeight > element.clientHeight + 1 && ['auto', 'scroll'].includes(style.overflowY)) ||
                (element.scrollWidth > element.clientWidth + 1 && ['auto', 'scroll'].includes(style.overflowX)));
            if (!element.matches(interactiveSelector) && !isPointerRoot && !isScrollable) continue;
            if (
              style.display === 'none' ||
              style.visibility === 'hidden' ||
              Number(style.opacity) <= 0.01 ||
              (rect.width === 0 && rect.height === 0) ||
              rect.right <= 0 ||
              rect.bottom <= 0 ||
              rect.left >= window.innerWidth ||
              rect.top >= window.innerHeight ||
              element.closest('[aria-hidden="true"],[inert]')
            ) {
              continue;
            }
            let clipped = false;
            for (let ancestor = element.parentElement; ancestor; ancestor = ancestor.parentElement) {
              const ancestorStyle = window.getComputedStyle(ancestor);
              const clipsX = ['auto', 'clip', 'hidden', 'scroll'].includes(ancestorStyle.overflowX);
              const clipsY = ['auto', 'clip', 'hidden', 'scroll'].includes(ancestorStyle.overflowY);
              if (!clipsX && !clipsY) continue;
              const ancestorRect = ancestor.getBoundingClientRect();
              if (
                (clipsX && (rect.right <= ancestorRect.left || rect.left >= ancestorRect.right)) ||
                (clipsY && (rect.bottom <= ancestorRect.top || rect.top >= ancestorRect.bottom))
              ) {
                clipped = true;
                break;
              }
            }
            if (clipped) continue;
            const ref = `e${firstRefNumber + elements.length}`;
            element.setAttribute(refAttribute, ref);
            elements.push({
              ref,
              tag: element.tagName.toLowerCase(),
              role: element.getAttribute('role'),
              type: element.getAttribute('type'),
              ariaLabel: element.getAttribute('aria-label'),
              placeholder: element.getAttribute('placeholder'),
              text: (element.innerText || element.textContent || '').slice(0, 1_000),
              scrollable: isScrollable,
            });
          }
          const loginRequired = Array.from(document.querySelectorAll<HTMLInputElement>('input[type="password"]')).some(
            (input) => {
              const style = window.getComputedStyle(input);
              const rect = input.getBoundingClientRect();
              return (
                style.display !== 'none' &&
                style.visibility !== 'hidden' &&
                Number(style.opacity) > 0.01 &&
                rect.width > 0 &&
                rect.height > 0
              );
            }
          );
          return {
            title: document.title,
            elements,
            bodyText: (document.body?.innerText ?? '').slice(0, maxTextLength),
            loginRequired,
          };
        },
        {
          interactiveSelector: INTERACTIVE_ELEMENT_SELECTOR,
          maxElements: remainingElements,
          maxTextLength: MAX_TEXT_LENGTH,
          refAttribute: BIWORK_REF_ATTRIBUTE,
          firstRefNumber: nextRefNumber,
        }
      );
      const text = compactText(snapshot.bodyText, MAX_TEXT_LENGTH);
      frames.push({
        frame,
        id: frameId,
        url: frame.url(),
        name: frame.name() || null,
        title: snapshot.title,
        text,
        elements: snapshot.elements,
        loginRequired: snapshot.loginRequired,
      });
      remainingElements -= snapshot.elements.length;
      nextRefNumber += snapshot.elements.length;
    }
    return { frames, text: combinedFrameText(frames) };
  }

  private async collectFrameText(page: Page): Promise<{
    frames: Array<Omit<FrameSnapshot, 'elements' | 'loginRequired'> & { element_count: number }>;
    text: string;
    truncated: boolean;
  }> {
    const frames: Array<Omit<FrameSnapshot, 'elements' | 'loginRequired'> & { element_count: number }> = [];
    for (const [frameIndex, frame] of page.frames().entries()) {
      // Keep frame ordering deterministic in the text returned to the model.
      // eslint-disable-next-line no-await-in-loop
      if (!(await this.isVisibleFrame(page, frame))) continue;
      // eslint-disable-next-line no-await-in-loop
      const value = await frame.evaluate(
        (maxLength) => ({
          title: document.title,
          bodyText: (document.body?.innerText ?? '').slice(0, maxLength),
        }),
        MAX_TEXT_LENGTH
      );
      frames.push({
        id: `f${frameIndex}`,
        url: frame.url(),
        name: frame.name() || null,
        title: value.title,
        text: compactText(value.bodyText, MAX_TEXT_LENGTH),
        element_count: 0,
      });
    }
    const unboundedText = frames
      .filter((frame) => frame.text)
      .map((frame) => (frame.id === 'f0' ? frame.text : `[Frame ${frame.id} ${frame.url}] ${frame.text}`))
      .join(' ');
    return {
      frames,
      text: compactText(unboundedText, MAX_TEXT_LENGTH),
      truncated: unboundedText.length > MAX_TEXT_LENGTH,
    };
  }

  private async isVisibleFrame(page: Page, frame: Frame): Promise<boolean> {
    if (frame === page.mainFrame()) return true;
    const frameElement = await frame.frameElement();
    try {
      return await frameElement.isVisible();
    } finally {
      await frameElement.dispose();
    }
  }

  private async waitForSpaSettled(page: Page): Promise<void> {
    try {
      await page.evaluate(
        ({ quietPeriodMs, timeoutMs }) =>
          new Promise<void>((resolve) => {
            let quietTimer: ReturnType<typeof setTimeout> | null = null;
            let timeoutTimer: ReturnType<typeof setTimeout> | null = null;
            const observer = new MutationObserver(() => scheduleQuiet());
            const finish = (): void => {
              observer.disconnect();
              if (quietTimer) clearTimeout(quietTimer);
              if (timeoutTimer) clearTimeout(timeoutTimer);
              resolve();
            };
            const scheduleQuiet = (): void => {
              if (quietTimer) clearTimeout(quietTimer);
              quietTimer = setTimeout(finish, quietPeriodMs);
            };
            observer.observe(document.documentElement, {
              attributes: true,
              childList: true,
              characterData: true,
              subtree: true,
            });
            scheduleQuiet();
            timeoutTimer = setTimeout(finish, timeoutMs);
          }),
        { quietPeriodMs: SPA_QUIET_PERIOD_MS, timeoutMs: SPA_SETTLE_TIMEOUT_MS }
      );
    } catch (error) {
      if (!isTransientPageStateError(error)) throw error;
    }
  }

  private async currentPageSignal(page: Page): Promise<string> {
    const title = await page.title().catch(() => '');
    const text = await page
      .locator('body')
      .innerText({ timeout: TEXT_EXTRACTION_TIMEOUT_MS })
      .catch(() => '');
    return pageSignal(page.url(), title, text);
  }

  private tabId(session: BrowserSession, page: Page): string {
    const existing = session.tabIds.get(page);
    if (existing) return existing;
    const tabId = `t${session.nextTabNumber}`;
    session.nextTabNumber += 1;
    session.tabIds.set(page, tabId);
    return tabId;
  }

  private openPages(session: BrowserSession): Page[] {
    return session.context.pages().filter((page) => !page.isClosed());
  }

  private requiredTab(session: BrowserSession, tabId: string): Page {
    const page = this.openPages(session).find((candidate) => this.tabId(session, candidate) === tabId);
    if (!page) {
      throw new BrowserExecutionError('BROWSER_TAB_NOT_FOUND', `Browser tab not found: ${tabId}`);
    }
    return page;
  }

  private async tabMetadata(session: BrowserSession, pages = this.openPages(session)): Promise<unknown[]> {
    return Promise.all(
      pages.map(async (page, index) => ({
        tab_id: this.tabId(session, page),
        index,
        url: page.url(),
        title: await page.title().catch(() => ''),
        active: page === session.page,
      }))
    );
  }

  private activePage(session: BrowserSession): Page {
    if (!session.page.isClosed()) return session.page;
    const fallback = lastOpenPage(session.context);
    if (!fallback) {
      throw new BrowserExecutionError('BROWSER_PAGE_NOT_FOUND', 'The browser session has no open pages');
    }
    session.page = fallback;
    session.refs.clear();
    return fallback;
  }

  private async withStablePage<T>(
    session: BrowserSession,
    operation: (page: Page) => Promise<T>
  ): Promise<{ page: Page; value: T }> {
    const deadline = Date.now() + PAGE_STABILITY_TIMEOUT_MS;
    const attempt = async (): Promise<{ page: Page; value: T }> => {
      const page = this.activePage(session);
      try {
        const value = await operation(page);
        if (page !== session.page) {
          if (Date.now() >= deadline) throw pageUnstableError();
          return attempt();
        }
        return { page, value };
      } catch (error) {
        if (!isTransientPageStateError(error)) throw error;
      }
      if (Date.now() >= deadline) throw pageUnstableError();
      const retryPage = this.activePage(session);
      await retryPage.waitForLoadState('domcontentloaded', { timeout: 2_000 }).catch((): void => undefined);
      await retryPage.waitForTimeout(PAGE_STABILITY_RETRY_DELAY_MS).catch((): void => undefined);
      return attempt();
    };
    return attempt();
  }

  private requiredSession(sessionId: string): BrowserSession {
    const session = this.sessions.get(sessionId);
    if (!session)
      throw new BrowserExecutionError('BROWSER_SESSION_NOT_FOUND', `Browser session not found: ${sessionId}`);
    return session;
  }

  private requiredRef(session: BrowserSession, ref: string): Locator {
    const locator = session.refs.get(ref);
    if (!locator) {
      throw new BrowserExecutionError('BROWSER_REF_STALE', `Unknown or stale browser element reference: ${ref}`);
    }
    return locator;
  }
}

function requiredString(value: unknown, field: string, allowEmpty = false): string {
  if (typeof value !== 'string' || (!allowEmpty && !value.trim())) {
    throw new BrowserExecutionError('BROWSER_WORK_INVALID', `${field} must be a string`);
  }
  return value;
}

function optionalString(value: unknown): string | null {
  return typeof value === 'string' && value.trim() ? value : null;
}

function optionalBoundedInteger(
  value: unknown,
  field: string,
  minimum: number,
  maximum: number,
  fallback: number
): number {
  if (value === undefined || value === null) return fallback;
  if (!Number.isInteger(value) || Number(value) < minimum || Number(value) > maximum) {
    throw new BrowserExecutionError(
      'BROWSER_WORK_INVALID',
      `${field} must be an integer between ${minimum} and ${maximum}`
    );
  }
  return Number(value);
}

function isTransientPageStateError(error: unknown): boolean {
  const message = error instanceof Error ? error.message : String(error);
  return /execution context was destroyed|cannot find context|frame was detached|target page.*closed|page has been closed/i.test(
    message
  );
}

function isClosedBrowserStateError(error: unknown): boolean {
  const message = error instanceof Error ? error.message : String(error);
  return /target page.*closed|page has been closed|context.*closed|browser has been closed/i.test(message);
}

function lastOpenPage(context: BrowserContext): Page | undefined {
  const pages = context.pages();
  for (let index = pages.length - 1; index >= 0; index -= 1) {
    if (!pages[index].isClosed()) return pages[index];
  }
  return undefined;
}

function pageUnstableError(): BrowserExecutionError {
  return new BrowserExecutionError(
    'BROWSER_PAGE_UNSTABLE',
    'The browser page kept navigating or switching tabs. Wait for it to settle, then take a new snapshot.'
  );
}

function optionalProfile(value: unknown): string {
  const profile = optionalString(value) ?? 'default';
  if (!/^[A-Za-z0-9_-]{1,64}$/.test(profile)) {
    throw new BrowserExecutionError('BROWSER_PROFILE_INVALID', 'Browser profile contains unsupported characters');
  }
  return profile;
}

function requiredHttpUrl(value: unknown): string {
  const text = requiredString(value, 'action.url');
  let url: URL;
  try {
    url = new URL(text);
  } catch {
    throw new BrowserExecutionError('BROWSER_URL_INVALID', 'Browser URL is invalid');
  }
  if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password) {
    throw new BrowserExecutionError('BROWSER_URL_INVALID', 'Only credential-free HTTP(S) browser URLs are allowed');
  }
  return url.toString();
}

function compactText(value: string, maxLength: number): string {
  return value.replace(/\s+/g, ' ').trim().slice(0, maxLength);
}

function pageSignal(url: string, title: string, text: string): string {
  return `${url}\n${title}\n${compactText(text, 4_000)}`;
}

function compactErrorCause(error: unknown): string {
  const message = error instanceof Error ? error.message : String(error);
  return compactText(message.split('\n')[0] ?? '', 500);
}

function combinedFrameText(frames: Array<Pick<FrameSnapshot, 'id' | 'url' | 'text'>>): string {
  const text = frames
    .filter((frame) => frame.text)
    .map((frame) => (frame.id === 'f0' ? frame.text : `[Frame ${frame.id} ${frame.url}] ${frame.text}`))
    .join(' ');
  return compactText(text, MAX_TEXT_LENGTH);
}

function resolveBrowserExecutablePath(): string | undefined {
  const configured = process.env.BIWORK_BROWSER_EXECUTABLE_PATH;
  if (configured) {
    if (!fs.existsSync(configured)) {
      throw new BrowserExecutionError('BROWSER_EXECUTABLE_NOT_FOUND', 'Configured browser executable does not exist');
    }
    return configured;
  }
  const candidates =
    process.platform === 'darwin'
      ? ['/Applications/Google Chrome.app/Contents/MacOS/Google Chrome']
      : process.platform === 'win32'
        ? [
            path.join(process.env.PROGRAMFILES ?? '', 'Google/Chrome/Application/chrome.exe'),
            path.join(process.env['PROGRAMFILES(X86)'] ?? '', 'Google/Chrome/Application/chrome.exe'),
            path.join(process.env.LOCALAPPDATA ?? '', 'Google/Chrome/Application/chrome.exe'),
          ]
        : ['/usr/bin/google-chrome-stable', '/usr/bin/google-chrome', '/usr/bin/chromium', '/usr/bin/chromium-browser'];
  return candidates.find((candidate) => candidate && fs.existsSync(candidate));
}
