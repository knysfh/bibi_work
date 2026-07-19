import type { ConfigKey, ConfigKeyMap } from './configKeys';
import {
  getAccessToken,
  getAuthorizationHeaders,
  peekAccessToken,
  requestWithAuthorizationRetry,
} from '../auth/authTokenBroker';
import { BackendHttpError } from '../adapter/httpBridge';

type Subscriber = (value: unknown) => void;

declare global {
  interface Window {
    __backendPort?: number;
  }

  // Main-process http callers do not have window. src/index.ts publishes the
  // renderer-facing gateway port here before any main-process bridge call.
  // eslint-disable-next-line no-var
  var __backendPort: number | undefined;
}

function getBaseUrl(): string {
  // WebUI browser mode: no preload, fetch same-origin so web-host's
  // static-server reverse-proxies /api/* to the backend.
  if (typeof window !== 'undefined' && typeof document !== 'undefined' && !(window as Window).__backendPort) {
    return '';
  }
  const port =
    typeof window !== 'undefined' ? (window as Window).__backendPort || 13400 : globalThis.__backendPort || 13400;
  return `http://127.0.0.1:${port}`;
}

async function fetchJson<T>(method: string, path: string, body?: unknown): Promise<T> {
  const url = `${getBaseUrl()}${path}`;
  const response = await requestWithAuthorizationRetry(async () => {
    const headers: Record<string, string> = {};
    if (body !== undefined) headers['Content-Type'] = 'application/json';
    Object.assign(headers, await getAuthorizationHeaders());
    return fetch(url, {
      method,
      headers,
      body: body !== undefined ? JSON.stringify(body) : undefined,
    });
  });
  if (!response.ok) {
    const rawText = await response.text().catch(() => '');
    let errorBody: unknown;
    try {
      errorBody = JSON.parse(rawText);
    } catch {
      errorBody = rawText;
    }
    throw new BackendHttpError({ method, path, status: response.status, body: errorBody });
  }
  const contentType = response.headers.get('Content-Type');
  if (!contentType?.includes('application/json')) {
    return undefined as T;
  }
  const json = await response.json();
  if (json && typeof json === 'object' && 'data' in json) {
    return json.data as T;
  }
  return json as T;
}

class ConfigServiceImpl {
  private cache = new Map<string, unknown>();
  private subscribers = new Map<string, Set<Subscriber>>();
  private initialized = false;
  private initializedForToken: string | null | undefined;
  private initPromise: Promise<void> | null = null;
  private initPromiseToken: string | null | undefined;
  private initSequence = 0;

  // Idempotent: concurrent callers share the same in-flight promise, and a
  // resolved init returns immediately. Modules that need persisted settings on
  // module load (theme/colorScheme/language) await whenReady() before reading.
  initialize(): Promise<void> {
    const tokenAtCall = peekAccessToken();
    if (this.initialized && this.initializedForToken === tokenAtCall) {
      return Promise.resolve();
    }
    if (this.initPromise && this.initPromiseToken === tokenAtCall) return this.initPromise;
    this.initPromiseToken = tokenAtCall;
    const sequence = ++this.initSequence;
    this.initPromise = (async () => {
      const token = await getAccessToken();
      if (sequence !== this.initSequence) return;
      if (!token) {
        this.cache.clear();
        this.initialized = true;
        this.initializedForToken = null;
        return;
      }
      const data = await fetchJson<Record<string, unknown>>('GET', '/api/settings/client');
      if (sequence !== this.initSequence) return;
      this.cache.clear();
      if (data) {
        for (const [key, value] of Object.entries(data)) {
          this.cache.set(key, value);
        }
      }
      // One-time theme migration: only when new keys are absent (idempotent).
      if (!this.cache.has('theme.activeId')) {
        const { migrateThemeConfig } = await import('@/common/theme/migrateThemeConfig');
        const migrated = migrateThemeConfig({
          theme: this.cache.get('theme') as string | undefined,
          'css.activeThemeId': this.cache.get('css.activeThemeId') as string | undefined,
          'css.themes': this.cache.get('css.themes') as never,
          customCss: this.cache.get('customCss') as string | undefined,
        });
        this.cache.set('theme.activeId', migrated['theme.activeId']);
        this.cache.set('theme.userThemes', migrated['theme.userThemes']);
        // Persist asynchronously; ignore failure (will re-run next launch).
        void fetchJson<void>('PUT', '/api/settings/client', migrated).catch(() => {});
      }
      this.initialized = true;
      this.initializedForToken = token;
    })();
    this.initPromise.catch(() => {
      // Allow a future caller to retry after a transient failure
      this.initPromise = null;
      this.initPromiseToken = undefined;
    });
    return this.initPromise;
  }

  whenReady(): Promise<void> {
    return this.initialize();
  }

  get<K extends ConfigKey>(key: K): ConfigKeyMap[K] | undefined {
    return this.cache.get(key) as ConfigKeyMap[K] | undefined;
  }

  async set<K extends ConfigKey>(key: K, value: ConfigKeyMap[K]): Promise<void> {
    this.cache.set(key, value);
    this.notify(key, value);
    await fetchJson<void>('PUT', '/api/settings/client', { [key]: value });
  }

  setLocal<K extends ConfigKey>(key: K, value: ConfigKeyMap[K]): void {
    this.cache.set(key, value);
    this.notify(key, value);
  }

  async remove(key: ConfigKey): Promise<void> {
    this.cache.delete(key);
    this.notify(key, undefined);
    await fetchJson<void>('PUT', '/api/settings/client', { [key]: null });
  }

  async setBatch(entries: Partial<{ [K in ConfigKey]: ConfigKeyMap[K] }>): Promise<void> {
    for (const [key, value] of Object.entries(entries)) {
      this.cache.set(key, value);
      this.notify(key as ConfigKey, value);
    }
    await fetchJson<void>('PUT', '/api/settings/client', entries);
  }

  subscribe(key: ConfigKey, callback: Subscriber): () => void {
    if (!this.subscribers.has(key)) {
      this.subscribers.set(key, new Set());
    }
    this.subscribers.get(key)!.add(callback);
    return () => {
      this.subscribers.get(key)?.delete(callback);
    };
  }

  isInitialized(): boolean {
    return this.initialized;
  }

  reset(): void {
    this.cache.clear();
    this.subscribers.clear();
    this.initialized = false;
    this.initializedForToken = undefined;
    this.initPromise = null;
    this.initPromiseToken = undefined;
    this.initSequence += 1;
  }

  clearSessionCache(): void {
    const removedKeys = Array.from(this.cache.keys()) as ConfigKey[];
    this.cache.clear();
    this.initialized = true;
    this.initializedForToken = null;
    this.initPromise = null;
    this.initPromiseToken = undefined;
    this.initSequence += 1;
    for (const key of removedKeys) {
      this.notify(key, undefined);
    }
  }

  private notify(key: ConfigKey, value: unknown): void {
    const subs = this.subscribers.get(key);
    if (subs) {
      for (const cb of subs) {
        cb(value);
      }
    }
  }
}

export const configService = new ConfigServiceImpl();
