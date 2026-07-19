/**
 * @vitest-environment node
 */

import { promises as fs } from 'node:fs';
import path from 'node:path';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { DesktopRefreshTokenStore } from '@process/auth/desktopRefreshTokenStore';

const testRoot = path.join('/tmp', 'biwork-refresh-token-store-tests');

afterEach(async () => {
  await fs.rm(testRoot, { recursive: true, force: true });
});

describe('DesktopRefreshTokenStore', () => {
  it('persists only encrypted ciphertext and restores the refresh token', async () => {
    const filePath = path.join(testRoot, 'auth', 'refresh-token.json');
    const encryption = {
      isAvailable: () => true,
      encrypt: (plaintext: string) => Buffer.from(`encrypted:${plaintext}`),
      decrypt: (encrypted: Buffer) => encrypted.toString().replace(/^encrypted:/, ''),
    };
    const store = new DesktopRefreshTokenStore(filePath, encryption);
    await store.save('refresh-secret');

    const raw = await fs.readFile(filePath, 'utf8');
    expect(raw).not.toContain('refresh-secret');
    await expect(new DesktopRefreshTokenStore(filePath, encryption).load()).resolves.toBe('refresh-secret');
    expect((await fs.stat(filePath)).mode & 0o777).toBe(0o600);
  });

  it('uses memory only when OS encryption is unavailable', async () => {
    const filePath = path.join(testRoot, 'refresh-token.json');
    const store = new DesktopRefreshTokenStore(filePath, {
      isAvailable: () => false,
      encrypt: vi.fn(),
      decrypt: vi.fn(),
    });

    await store.save('memory-refresh');
    await expect(store.load()).resolves.toBe('memory-refresh');
    await expect(fs.stat(filePath)).rejects.toMatchObject({ code: 'ENOENT' });
  });

  it('removes persisted and in-memory credentials on clear', async () => {
    const filePath = path.join(testRoot, 'refresh-token.json');
    const encryption = {
      isAvailable: () => true,
      encrypt: (plaintext: string) => Buffer.from(plaintext),
      decrypt: (encrypted: Buffer) => encrypted.toString(),
    };
    const store = new DesktopRefreshTokenStore(filePath, encryption);
    await store.save('refresh-secret');
    await store.clear();

    await expect(store.load()).resolves.toBeNull();
    await expect(fs.stat(filePath)).rejects.toMatchObject({ code: 'ENOENT' });
  });
});
