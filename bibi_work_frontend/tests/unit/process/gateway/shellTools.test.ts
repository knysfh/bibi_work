import { describe, expect, it } from 'vitest';
import { validateExternalUrl } from '@process/gateway/shellTools';

describe('validateExternalUrl', () => {
  it.each([
    ['https://example.com/docs?q=1', 'https://example.com/docs?q=1'],
    ['http://127.0.0.1:3100/path', 'http://127.0.0.1:3100/path'],
    ['mailto:team@example.com', 'mailto:team@example.com'],
  ])('allows explicitly supported URLs', (input, expected) => {
    expect(validateExternalUrl(input)).toBe(expected);
  });

  it.each(['file:///etc/passwd', 'javascript:alert(1)', 'vscode://file/tmp/project', 'not a url'])(
    'rejects unsafe or invalid external URLs',
    (input) => {
      expect(() => validateExternalUrl(input)).toThrow();
    }
  );

  it('rejects credentials embedded in web URLs', () => {
    expect(() => validateExternalUrl('https://user:secret@example.com/path')).toThrow(
      'external URL credentials are not allowed'
    );
  });
});
