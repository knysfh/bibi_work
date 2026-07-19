import { describe, expect, it } from 'vitest';
import {
  buildOidcDeepLinkFromCallbackUrl,
  parseOidcCallbackFromUrl,
  startOidcLoopbackServer,
  type OidcLoopbackCallback,
} from '@process/utils/oidcLoopback';

describe('desktop OIDC loopback callback', () => {
  it('converts authorization callback parameters into an biwork deep link', () => {
    const deepLink = buildOidcDeepLinkFromCallbackUrl('/callback?code=code-1&state=state-1');

    expect(deepLink).toBe('biwork://auth/callback?code=code-1&state=state-1');
  });

  it('passes OIDC error callbacks through the same deep link path', () => {
    const deepLink = buildOidcDeepLinkFromCallbackUrl('/callback?error=access_denied&error_description=Denied');

    expect(deepLink).toBe('biwork://auth/callback?error=access_denied&error_description=Denied');
  });

  it('rejects unrelated loopback paths and empty callbacks', () => {
    expect(buildOidcDeepLinkFromCallbackUrl('/other?code=code-1&state=state-1')).toBeNull();
    expect(buildOidcDeepLinkFromCallbackUrl('/callback?state=state-1')).toBeNull();
  });

  it('parses callback parameters for main-process OIDC handling', () => {
    expect(parseOidcCallbackFromUrl('/callback?code=code-1&state=state-1')).toEqual({
      code: 'code-1',
      state: 'state-1',
    });
  });

  it('can hand the loopback callback directly to the main process', async () => {
    const received: OidcLoopbackCallback[] = [];
    const handle = await startOidcLoopbackServer({
      port: 0,
      onCallback: (callback) => {
        received.push(callback);
      },
    });

    try {
      const response = await fetch(`http://127.0.0.1:${handle.port}/callback?code=code-1&state=state-1`);
      expect(response.status).toBe(200);
      expect(await response.text()).toContain('Authentication complete');
      expect(received).toEqual([{ code: 'code-1', state: 'state-1' }]);
    } finally {
      await handle.stop();
    }
  });
});
