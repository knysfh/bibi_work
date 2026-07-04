export interface TokenSet {
  accessToken: string;
  refreshToken?: string;
  expiresAt?: string;
}

export interface TokenProvider {
  getAccessToken(): Promise<string | null>;
  setTokenSet(tokenSet: TokenSet): Promise<void>;
  clearTokenSet(): Promise<void>;
}

const TOKEN_KEY = "bibi_work.token_set";

export function createBrowserTokenProvider(
  storage: Storage = window.sessionStorage
): TokenProvider {
  return {
    async getAccessToken() {
      const raw = storage.getItem(TOKEN_KEY);
      if (!raw) {
        return null;
      }
      try {
        const parsed = JSON.parse(raw) as Partial<TokenSet>;
        return typeof parsed.accessToken === "string" ? parsed.accessToken : null;
      } catch {
        storage.removeItem(TOKEN_KEY);
        return null;
      }
    },
    async setTokenSet(tokenSet) {
      storage.setItem(TOKEN_KEY, JSON.stringify(tokenSet));
    },
    async clearTokenSet() {
      storage.removeItem(TOKEN_KEY);
    }
  };
}
