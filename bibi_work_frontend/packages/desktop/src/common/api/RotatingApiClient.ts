import { ApiKeyManager } from './ApiKeyManager';
import type { AuthType } from '../utils/platformAuthType';

// Unified interface for chat completion across different providers
export interface UnifiedChatCompletionParams {
  model: string;
  messages: unknown; // Allow flexible message formats for compatibility
}

export interface UnifiedChatCompletionResponse {
  id: string;
  object: string;
  created: number;
  model: string;
  choices: Array<{
    index: number;
    message: {
      role: string;
      content: string;
      images?: Array<{
        type: 'image_url';
        image_url: { url: string };
      }>;
    };
    finish_reason: string;
  }>;
  usage?: {
    prompt_tokens: number;
    completion_tokens: number;
    total_tokens: number;
  };
}

export interface RotatingApiClientOptions {
  /** Number of retries after the original request. */
  maxRetries?: number;
}

// Constants for better maintainability
const DEFAULT_MAX_RETRIES = 3;
const RETRY_DELAYS_MS = [5000, 15000, 30000] as const;

export interface ApiError extends Error {
  status?: number;
  code?: number;
}

export abstract class RotatingApiClient<T> {
  protected apiKeyManager?: ApiKeyManager;
  protected client?: T;
  protected readonly createClientFn: (api_key: string) => T;
  protected readonly options: Required<RotatingApiClientOptions>;
  protected readonly originalApiKeys: string;

  constructor(
    api_keys: string,
    authType: AuthType,
    createClientFn: (api_key: string) => T,
    options: RotatingApiClientOptions = {}
  ) {
    this.originalApiKeys = api_keys;
    this.createClientFn = createClientFn;
    this.options = {
      maxRetries: options.maxRetries ?? DEFAULT_MAX_RETRIES,
    };

    if (api_keys && (api_keys.includes(',') || api_keys.includes('\n'))) {
      this.apiKeyManager = new ApiKeyManager(api_keys, authType);
    }

    this.initializeClient();
  }

  protected initializeClient(): void {
    const api_key = this.getCurrentApiKey();

    if (api_key) {
      try {
        this.client = this.createClientFn(api_key);
      } catch (error) {
        console.error('[RotatingApiClient] Client initialization failed:', error);
        throw error;
      }
    }
  }

  protected getCurrentApiKey(): string | undefined {
    if (this.apiKeyManager?.hasMultipleKeys()) {
      return this.apiKeyManager.getCurrentKey();
    }
    // For single key case, extract the first key
    return this.extractFirstKey();
  }

  private extractFirstKey(): string | undefined {
    if (!this.originalApiKeys) return undefined;

    if (this.isSingleKey()) {
      return this.originalApiKeys.trim() || undefined;
    }

    const keys = this.parseMultipleKeys();
    return keys[0] || undefined;
  }

  private isSingleKey(): boolean {
    return !this.originalApiKeys.includes(',') && !this.originalApiKeys.includes('\n');
  }

  private parseMultipleKeys(): string[] {
    return this.originalApiKeys
      .split(/[,\n]/)
      .map((key) => key.trim())
      .filter((key) => key);
  }

  protected isRetryableError(error: unknown): boolean {
    if (!error || typeof error !== 'object') return false;

    const apiError = error as ApiError;
    const status = apiError.status || apiError.code;

    // Authentication errors do not recover by waiting. They may still rotate to
    // another configured key, but a single invalid key must fail immediately.
    return status === 408 || status === 409 || status === 429 || (status >= 500 && status < 600);
  }

  protected isKeyRotationError(error: unknown): boolean {
    if (!error || typeof error !== 'object') return false;
    const apiError = error as ApiError;
    const status = apiError.status || apiError.code;
    return status === 401 || this.isRetryableError(error);
  }

  protected delay(ms: number): Promise<void> {
    return new Promise((resolve) => setTimeout(resolve, ms));
  }

  async executeWithRetry<R>(operation: (client: T) => Promise<R>): Promise<R> {
    if (!this.client) {
      throw new Error('Client not initialized - no valid API key provided');
    }

    let lastError: unknown;

    for (let attempt = 0; attempt <= this.options.maxRetries; attempt++) {
      try {
        return await operation(this.client);
      } catch (error) {
        lastError = error;

        const isLastAttempt = attempt === this.options.maxRetries;
        const canRotateKey = this.apiKeyManager?.hasMultipleKeys() && this.isKeyRotationError(error) && !isLastAttempt;

        if (canRotateKey && this.apiKeyManager.rotateKey()) {
          this.initializeClient();
          await this.delay(RETRY_DELAYS_MS[Math.min(attempt, RETRY_DELAYS_MS.length - 1)]);
          continue;
        }

        if (!this.isRetryableError(error) || isLastAttempt) {
          break;
        }

        // Regular retry with delay
        await this.delay(RETRY_DELAYS_MS[Math.min(attempt, RETRY_DELAYS_MS.length - 1)]);
      }
    }

    throw lastError;
  }

  hasMultipleKeys(): boolean {
    return this.apiKeyManager?.hasMultipleKeys() ?? false;
  }

  getKeyStatus() {
    return this.apiKeyManager?.getStatus() ?? null;
  }
}
