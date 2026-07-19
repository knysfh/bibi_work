import { promises as fs } from 'node:fs';
import path from 'node:path';

export type RefreshTokenEncryption = {
  decrypt: (encrypted: Buffer) => string;
  encrypt: (plaintext: string) => Buffer;
  isAvailable: () => boolean;
};

type StoredRefreshToken = {
  ciphertext: string;
  version: 1;
};

export class DesktopRefreshTokenStore {
  private memoryToken: string | null = null;
  private writeChain: Promise<void> = Promise.resolve();

  constructor(
    private readonly filePath: string,
    private readonly encryption: RefreshTokenEncryption,
    private readonly warn: (message: string, error?: unknown) => void = console.warn
  ) {}

  async load(): Promise<string | null> {
    if (this.memoryToken) return this.memoryToken;
    if (!this.encryption.isAvailable()) return null;
    try {
      const raw = await fs.readFile(this.filePath, 'utf8');
      const stored = JSON.parse(raw) as Partial<StoredRefreshToken>;
      if (stored.version !== 1 || typeof stored.ciphertext !== 'string' || !stored.ciphertext) {
        throw new Error('refresh token store payload is invalid');
      }
      const token = this.encryption.decrypt(Buffer.from(stored.ciphertext, 'base64')).trim();
      this.memoryToken = token || null;
      return this.memoryToken;
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== 'ENOENT') {
        this.warn('[auth] failed to load encrypted refresh token', error);
      }
      return null;
    }
  }

  async save(token: string): Promise<void> {
    const normalized = token.trim();
    this.memoryToken = normalized || null;
    if (!this.memoryToken || !this.encryption.isAvailable()) return;
    const tokenToPersist = this.memoryToken;
    this.writeChain = this.writeChain.then(async () => {
      const encrypted = this.encryption.encrypt(tokenToPersist);
      const payload: StoredRefreshToken = { version: 1, ciphertext: encrypted.toString('base64') };
      await fs.mkdir(path.dirname(this.filePath), { recursive: true, mode: 0o700 });
      const temporaryPath = `${this.filePath}.tmp`;
      await fs.writeFile(temporaryPath, `${JSON.stringify(payload)}\n`, { encoding: 'utf8', mode: 0o600 });
      await fs.rename(temporaryPath, this.filePath);
      await fs.chmod(this.filePath, 0o600).catch(() => {});
    });
    await this.writeChain;
  }

  async clear(): Promise<void> {
    this.memoryToken = null;
    this.writeChain = this.writeChain.then(async () => {
      await fs.unlink(this.filePath).catch((error: NodeJS.ErrnoException) => {
        if (error.code !== 'ENOENT') throw error;
      });
    });
    await this.writeChain;
  }
}
