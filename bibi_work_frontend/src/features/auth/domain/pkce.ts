export interface PkcePair {
  verifier: string;
  challenge: string;
  state: string;
}

export async function createPkcePair(): Promise<PkcePair> {
  const verifier = randomUrlSafeString(64);
  const state = randomUrlSafeString(32);
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(verifier));
  return {
    verifier,
    state,
    challenge: base64UrlEncode(new Uint8Array(digest))
  };
}

export function buildAuthorizationUrl(input: {
  authorizationEndpoint: string;
  clientId: string;
  redirectUri: string;
  scope?: string;
  pkce: PkcePair;
}): string {
  const url = new URL(input.authorizationEndpoint);
  url.searchParams.set("response_type", "code");
  url.searchParams.set("client_id", input.clientId);
  url.searchParams.set("redirect_uri", input.redirectUri);
  url.searchParams.set("scope", input.scope ?? "openid profile email");
  url.searchParams.set("state", input.pkce.state);
  url.searchParams.set("code_challenge", input.pkce.challenge);
  url.searchParams.set("code_challenge_method", "S256");
  return url.toString();
}

function randomUrlSafeString(bytes: number): string {
  const values = new Uint8Array(bytes);
  crypto.getRandomValues(values);
  return base64UrlEncode(values);
}

function base64UrlEncode(values: Uint8Array): string {
  let binary = "";
  for (const value of values) {
    binary += String.fromCharCode(value);
  }
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/g, "");
}
