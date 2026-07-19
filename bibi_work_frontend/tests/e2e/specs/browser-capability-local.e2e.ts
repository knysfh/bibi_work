import { expect, test } from '@playwright/test';
import http from 'node:http';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import {
  BrowserExecutionError,
  BrowserSessionManager,
} from '../../../packages/desktop/src/process/browser/browserSessionManager';

test('local browser executor keeps a profile, snapshots the page, and blocks password fill', async () => {
  test.setTimeout(120_000);
  const server = http.createServer((request, response) => {
    response.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8' });
    if (request.url === '/oa') {
      response.end('<!doctype html><html><head><title>OA Fixture</title></head><body>OA ready</body></html>');
      return;
    }
    response.end(`<!doctype html>
      <html><head><title>BiWork Browser Fixture</title>
        <style>
          .mail-cell { cursor: pointer; }
          .check-col { position: relative; overflow: hidden; }
          .rc-hidden { position: absolute; left: -14000px; }
        </style>
      </head>
      <body>
        <h1>OA Login Fixture</h1>
        <label>Account <input aria-label="Account" /></label>
        <label>Password <input aria-label="Password" type="password" /></label>
        <button type="button">Sign in</button>
        <button type="button" onclick="window.open('/oa', '_blank')">Open OA</button>
        <table><tbody><tr>
          <td class="check-col"><input class="rc-hidden" name="mid" type="checkbox" /></td>
          <td class="mail-cell">Attendance alert</td>
        </tr></tbody></table>
        <script>
          document.querySelector('.mail-cell').addEventListener('click', () => {
            document.title = 'Attendance opened';
          });
        </script>
      </body></html>`);
  });
  await new Promise<void>((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', resolve);
  });
  const address = server.address();
  if (!address || typeof address === 'string') throw new Error('fixture server did not bind a TCP port');
  const profilesDirectory = await fs.mkdtemp(path.join(os.tmpdir(), 'biwork-browser-e2e-'));
  const manager = new BrowserSessionManager({ profilesDirectory, headless: true });

  try {
    const opened = await manager.execute({
      protocol: 'biwork_browser.v1',
      kind: 'browser',
      session_id: 'browser-e2e-session',
      profile: 'oa-fixture',
      action: { name: 'open', url: `http://127.0.0.1:${address.port}/login` },
    });
    expect(opened).toMatchObject({
      kind: 'browser',
      action: 'open',
      session_id: 'browser-e2e-session',
      profile: 'oa-fixture',
      title: 'BiWork Browser Fixture',
    });
    const elements = opened.elements as Array<{ ref?: string; type?: string; label?: string }>;
    expect(elements.some((element) => element.type === 'checkbox')).toBe(false);
    const attendance = elements.find((element) => element.label === 'Attendance alert');
    expect(attendance?.ref).toBeTruthy();
    const clicked = await manager.execute({
      protocol: 'biwork_browser.v1',
      kind: 'browser',
      session_id: 'browser-e2e-session',
      action: { name: 'click', ref: attendance!.ref },
    });
    expect(clicked).toMatchObject({ action: 'click', title: 'Attendance opened' });
    const account = elements.find((element) => element.label === 'Account');
    expect(account?.ref).toBeTruthy();
    const extracted = await manager.execute({
      protocol: 'biwork_browser.v1',
      kind: 'browser',
      session_id: 'browser-e2e-session',
      action: { name: 'extract_text', ref: account!.ref },
    });
    expect(extracted).toMatchObject({ action: 'extract_text', text: '' });
    const password = elements.find((element) => element.type === 'password');
    expect(password?.ref).toBeTruthy();

    let blocked: unknown;
    try {
      await manager.execute({
        protocol: 'biwork_browser.v1',
        kind: 'browser',
        session_id: 'browser-e2e-session',
        action: { name: 'fill', ref: password!.ref, text: 'must-not-enter-runtime' },
      });
    } catch (error) {
      blocked = error;
    }
    expect(blocked).toBeInstanceOf(BrowserExecutionError);
    expect(blocked).toMatchObject({ code: 'BROWSER_SENSITIVE_INPUT_BLOCKED' });

    const openOa = elements.find((element) => element.label === 'Open OA');
    expect(openOa?.ref).toBeTruthy();
    const popup = await manager.execute({
      protocol: 'biwork_browser.v1',
      kind: 'browser',
      session_id: 'browser-e2e-session',
      action: { name: 'click', ref: openOa!.ref },
    });
    expect(popup).toMatchObject({ action: 'click', title: 'OA Fixture', tab_count: 2 });
    expect(String(popup.url)).toContain('/oa');
    const tabs = await manager.execute({
      protocol: 'biwork_browser.v1',
      kind: 'browser',
      session_id: 'browser-e2e-session',
      action: { name: 'tab_list' },
    });
    expect(tabs).toMatchObject({ action: 'tab_list', tab_count: 2 });
    const tabItems = tabs.tabs as Array<{ tab_id: string; active: boolean; url: string }>;
    const openerTab = tabItems.find((tab) => tab.url.includes('/login'));
    const popupTab = tabItems.find((tab) => tab.url.includes('/oa'));
    expect(openerTab?.tab_id).toBeTruthy();
    expect(popupTab).toMatchObject({ active: true });

    const selected = await manager.execute({
      protocol: 'biwork_browser.v1',
      kind: 'browser',
      session_id: 'browser-e2e-session',
      action: { name: 'tab_select', tab_id: openerTab!.tab_id },
    });
    expect(selected).toMatchObject({ action: 'tab_select', tab_id: openerTab!.tab_id, tab_count: 2 });
    const tabClosed = await manager.execute({
      protocol: 'biwork_browser.v1',
      kind: 'browser',
      session_id: 'browser-e2e-session',
      action: { name: 'tab_close', tab_id: popupTab!.tab_id },
    });
    expect(tabClosed).toMatchObject({ action: 'tab_close', tab_id: openerTab!.tab_id, tab_count: 1 });

    const closed = await manager.execute({
      protocol: 'biwork_browser.v1',
      kind: 'browser',
      session_id: 'browser-e2e-session',
      action: { name: 'close' },
    });
    expect(closed).toMatchObject({ closed: true });
  } finally {
    await manager.closeAll();
    await new Promise<void>((resolve) => server.close(() => resolve()));
    await fs.rm(profilesDirectory, { recursive: true, force: true });
  }
});

test('local browser executor handles SPA changes, virtual lists, and expired login state', async () => {
  test.setTimeout(120_000);
  const server = http.createServer((_request, response) => {
    response.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8' });
    response.end(`<!doctype html>
      <html><head><title>Browser State Fixture</title>
        <style>
          #work-hours { height: 140px; overflow-y: auto; border: 1px solid #999; position: relative; }
          #spacer { height: 1600px; }
          #visible-rows { position: absolute; left: 0; right: 0; top: 0; }
          .row { height: 40px; }
        </style>
      </head>
      <body>
        <h1>Authenticated dashboard</h1>
        <button id="load-report" type="button">Load SPA report</button>
        <div id="spa-result">Report pending</div>
        <div id="work-hours" aria-label="Work hours virtual list">
          <div id="spacer"></div><div id="visible-rows"></div>
        </div>
        <button id="expire-session" type="button">Expire login</button>
        <script>
          const list = document.querySelector('#work-hours');
          const rows = document.querySelector('#visible-rows');
          function renderRows() {
            const start = Math.floor(list.scrollTop / 40) + 1;
            rows.style.transform = 'translateY(' + ((start - 1) * 40) + 'px)';
            rows.innerHTML = Array.from({ length: 5 }, (_, index) =>
              '<div class="row">Work hour ' + (start + index) + '</div>'
            ).join('');
          }
          list.addEventListener('scroll', renderRows);
          renderRows();
          document.querySelector('#load-report').addEventListener('click', () => {
            setTimeout(() => {
              document.querySelector('#spa-result').textContent = 'SPA report loaded';
            }, 600);
          });
          document.querySelector('#expire-session').addEventListener('click', () => {
            history.pushState({}, '', '/login');
            document.body.innerHTML = '<h1>Session expired</h1><label>Password <input type="password" /></label>';
            document.title = 'Login required';
          });
        </script>
      </body></html>`);
  });
  await new Promise<void>((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', resolve);
  });
  const address = server.address();
  if (!address || typeof address === 'string') throw new Error('browser state fixture did not bind a TCP port');
  const profilesDirectory = await fs.mkdtemp(path.join(os.tmpdir(), 'biwork-browser-state-e2e-'));
  const manager = new BrowserSessionManager({ profilesDirectory, headless: true });

  try {
    const opened = await manager.execute({
      protocol: 'biwork_browser.v1',
      kind: 'browser',
      session_id: 'browser-state-session',
      profile: 'browser-state-fixture',
      action: { name: 'open', url: `http://127.0.0.1:${address.port}/dashboard` },
    });
    expect(opened).toMatchObject({ auth_state: 'authenticated_or_public', auth_expired: false });
    const openedElements = opened.elements as Array<{ ref: string; label: string; scrollable: boolean }>;
    const loadReport = openedElements.find((element) => element.label === 'Load SPA report');
    const virtualList = openedElements.find(
      (element) => element.label === 'Work hours virtual list' && element.scrollable
    );
    const expireSession = openedElements.find((element) => element.label === 'Expire login');
    expect(loadReport?.ref).toBeTruthy();
    expect(virtualList?.ref).toBeTruthy();
    expect(expireSession?.ref).toBeTruthy();

    const clicked = await manager.execute({
      protocol: 'biwork_browser.v1',
      kind: 'browser',
      session_id: 'browser-state-session',
      action: { name: 'click', ref: loadReport!.ref },
    });
    expect(String(clicked.text)).toContain('Report pending');
    const changed = await manager.execute({
      protocol: 'biwork_browser.v1',
      kind: 'browser',
      session_id: 'browser-state-session',
      action: { name: 'wait_for_change', timeout_ms: 3_000 },
    });
    expect(String(changed.text)).toContain('SPA report loaded');

    const currentElements = changed.elements as Array<{ ref: string; label: string; scrollable: boolean }>;
    const currentList = currentElements.find(
      (element) => element.label === 'Work hours virtual list' && element.scrollable
    );
    const currentExpire = currentElements.find((element) => element.label === 'Expire login');
    const scrolled = await manager.execute({
      protocol: 'biwork_browser.v1',
      kind: 'browser',
      session_id: 'browser-state-session',
      action: { name: 'scroll', ref: currentList!.ref, delta_y: 720 },
    });
    expect(String(scrolled.text)).toContain('Work hour 19');

    const refreshedElements = scrolled.elements as Array<{ ref: string; label: string }>;
    const refreshedExpire = refreshedElements.find((element) => element.label === currentExpire!.label);
    const expired = await manager.execute({
      protocol: 'biwork_browser.v1',
      kind: 'browser',
      session_id: 'browser-state-session',
      action: { name: 'click', ref: refreshedExpire!.ref },
    });
    expect(expired).toMatchObject({
      auth_state: 'login_required',
      auth_expired: true,
      title: 'Login required',
    });
    expect(String(expired.url)).toContain('/login');
  } finally {
    await manager.closeAll();
    await new Promise<void>((resolve) => server.close(() => resolve()));
    await fs.rm(profilesDirectory, { recursive: true, force: true });
  }
});

test('local browser executor snapshots and operates visible cross-origin iframe content', async () => {
  test.setTimeout(120_000);
  const frameServer = http.createServer((_request, response) => {
    response.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8' });
    response.end(`<!doctype html>
      <html><head><title>Attendance Frame</title></head>
      <body>
        <h1>Attendance dashboard</h1>
        <button type="button" aria-label="View attendance details">View details</button>
        <div id="result">No attendance selected</div>
        <script>
          document.querySelector('button').addEventListener('click', () => {
            document.querySelector('#result').textContent = 'Attendance details loaded';
          });
        </script>
      </body></html>`);
  });
  await new Promise<void>((resolve, reject) => {
    frameServer.once('error', reject);
    frameServer.listen(0, '127.0.0.1', resolve);
  });
  const frameAddress = frameServer.address();
  if (!frameAddress || typeof frameAddress === 'string') {
    throw new Error('iframe fixture server did not bind a TCP port');
  }

  const pageServer = http.createServer((_request, response) => {
    response.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8' });
    response.end(`<!doctype html>
      <html><head><title>OA Shell</title></head>
      <body>
        <h1>OA navigation</h1>
        <iframe title="Attendance" style="width:800px;height:600px" src="http://127.0.0.1:${frameAddress.port}/attendance"></iframe>
      </body></html>`);
  });
  await new Promise<void>((resolve, reject) => {
    pageServer.once('error', reject);
    pageServer.listen(0, '127.0.0.1', resolve);
  });
  const pageAddress = pageServer.address();
  if (!pageAddress || typeof pageAddress === 'string') {
    throw new Error('page fixture server did not bind a TCP port');
  }

  const profilesDirectory = await fs.mkdtemp(path.join(os.tmpdir(), 'biwork-browser-iframe-e2e-'));
  const manager = new BrowserSessionManager({ profilesDirectory, headless: true });

  try {
    const opened = await manager.execute({
      protocol: 'biwork_browser.v1',
      kind: 'browser',
      session_id: 'browser-iframe-session',
      profile: 'oa-iframe-fixture',
      action: { name: 'open', url: `http://127.0.0.1:${pageAddress.port}/oa` },
    });
    expect(opened).toMatchObject({ title: 'OA Shell', frame_count: 2 });
    expect(String(opened.text)).toContain('Attendance dashboard');
    expect(String(opened.text)).toContain('No attendance selected');

    const frames = opened.frames as Array<{ id: string; url: string; element_count: number }>;
    expect(frames).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ id: 'f0', element_count: 0 }),
        expect.objectContaining({ url: expect.stringContaining(`:${frameAddress.port}/attendance`) }),
      ])
    );
    const elements = opened.elements as Array<{
      ref?: string;
      label?: string;
      frame_id?: string;
      frame_url?: string;
    }>;
    const attendanceDetails = elements.find((element) => element.label === 'View attendance details');
    expect(attendanceDetails).toMatchObject({ frame_id: 'f1' });
    expect(attendanceDetails?.frame_url).toContain(`:${frameAddress.port}/attendance`);

    const clicked = await manager.execute({
      protocol: 'biwork_browser.v1',
      kind: 'browser',
      session_id: 'browser-iframe-session',
      action: { name: 'click', ref: attendanceDetails!.ref },
    });
    expect(String(clicked.text)).toContain('Attendance details loaded');

    const extracted = await manager.execute({
      protocol: 'biwork_browser.v1',
      kind: 'browser',
      session_id: 'browser-iframe-session',
      action: { name: 'extract_text' },
    });
    expect(extracted).toMatchObject({ action: 'extract_text', frame_count: 2 });
    expect(String(extracted.text)).toContain('Attendance details loaded');
  } finally {
    await manager.closeAll();
    await Promise.all([
      new Promise<void>((resolve) => pageServer.close(() => resolve())),
      new Promise<void>((resolve) => frameServer.close(() => resolve())),
    ]);
    await fs.rm(profilesDirectory, { recursive: true, force: true });
  }
});
