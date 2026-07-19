import { describe, expect, it } from 'vitest';
import { resolveOfficeWatchProxyTarget } from '@process/gateway/officeProxy';

describe('resolveOfficeWatchProxyTarget', () => {
  it('maps approved proxy routes to loopback while preserving path and query', () => {
    const target = resolveOfficeWatchProxyTarget(
      new URL('http://desktop/api/office-watch-proxy/43120/assets/index.js?v=2')
    );

    expect(target.toString()).toBe('http://127.0.0.1:43120/assets/index.js?v=2');
  });

  it('uses the loopback root for a bare port route', () => {
    const target = resolveOfficeWatchProxyTarget(new URL('http://desktop/api/ppt-proxy/8080'));
    expect(target.toString()).toBe('http://127.0.0.1:8080/');
  });

  it.each([
    'http://desktop/api/ppt-proxy/0',
    'http://desktop/api/ppt-proxy/65536',
    'http://desktop/api/ppt-proxy/not-a-port',
    'http://desktop/api/office-watch-proxy/8080.evil/path',
  ])('rejects invalid proxy targets', (input) => {
    expect(() => resolveOfficeWatchProxyTarget(new URL(input))).toThrow();
  });
});
