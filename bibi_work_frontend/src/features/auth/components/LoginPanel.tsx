import { ExternalLink, KeyRound, Server } from "lucide-react";
import { useState } from "react";
import { usePlatformApi } from "../../../app/providers";
import { useI18n } from "../../../shared/i18n";
import { Button, TextArea } from "../../../shared/ui";
import { useOidcConfigQuery } from "../api/auth.queries";
import { buildAuthorizationUrl, createPkcePair } from "../domain/pkce";

export function LoginPanel({ onTokenSaved }: { onTokenSaved: () => void }) {
  const { apiBaseUrl, authApi, desktopAuthApi, tokenProvider } = usePlatformApi();
  const { t } = useI18n();
  const oidcConfig = useOidcConfigQuery();
  const [token, setToken] = useState("");
  const [message, setMessage] = useState<string | null>(null);
  const [oidcLoginPending, setOidcLoginPending] = useState(false);

  async function saveToken() {
    const accessToken = extractAccessToken(token);
    if (!accessToken) {
      setMessage(t("auth.needToken"));
      return;
    }
    await saveAndVerifyTokenSet({ accessToken });
  }

  async function openOidcLogin() {
    if (!oidcConfig.data?.authorizationEndpoint) {
      setMessage(t("auth.missingAuthorizationEndpoint"));
      return;
    }
    if (!oidcConfig.data.tokenEndpoint) {
      setMessage(t("auth.missingTokenEndpoint"));
      return;
    }
    const pkce = await createPkcePair();
    const clientId = import.meta.env.VITE_FERRISKEY_CLIENT_ID ?? "bibi-work-desktop";
    const redirectUri =
      import.meta.env.VITE_BIBI_WORK_REDIRECT_URI ?? "bibi-work://auth/callback";
    const url = buildAuthorizationUrl({
      authorizationEndpoint: oidcConfig.data.authorizationEndpoint,
      clientId,
      redirectUri,
      pkce
    });
    setOidcLoginPending(true);
    setMessage(t("auth.loginOpened"));
    try {
      const tokenSet = await desktopAuthApi.loginWithOidc({
        authorizationUrl: url,
        tokenEndpoint: oidcConfig.data.tokenEndpoint,
        clientId,
        redirectUri,
        codeVerifier: pkce.verifier,
        state: pkce.state
      });
      await saveAndVerifyTokenSet(tokenSet);
    } catch (error) {
      setMessage(error instanceof Error ? error.message : String(error));
    } finally {
      setOidcLoginPending(false);
    }
  }

  async function saveAndVerifyTokenSet(tokenSet: { accessToken: string; refreshToken?: string }) {
    try {
      await tokenProvider.setTokenSet(tokenSet);
      await authApi.getMe();
      setToken("");
      onTokenSaved();
    } catch (error) {
      await tokenProvider.clearTokenSet();
      setMessage(error instanceof Error ? error.message : String(error));
    }
  }

  return (
    <div className="login-screen">
      <section className="login-panel">
        <div className="login-title">
          <KeyRound size={24} />
          <div>
            <h1>Bibi Work</h1>
            <p>{t("auth.subtitle")}</p>
          </div>
        </div>
        <div className="login-meta">
          <Server size={16} />
          <span>{apiBaseUrl}</span>
        </div>
        <div className="login-actions">
          <Button
            variant="primary"
            icon={<ExternalLink size={16} />}
            onClick={openOidcLogin}
            disabled={oidcConfig.isLoading || oidcLoginPending}
          >
            {t("auth.openFerriskey")}
          </Button>
          <span title={oidcConfig.data?.issuer}>
            {oidcConfig.data?.issuer ? t("auth.oidcReady") : t("auth.waitingOidc")}
          </span>
        </div>
        <details className="login-dev-token">
          <summary>{t("auth.devTokenToggle")}</summary>
          <div className="login-dev-token-body">
            <p>{t("auth.devTokenHint")}</p>
            <label className="field-stack">
              <span>{t("auth.devToken")}</span>
              <TextArea
                rows={5}
                value={token}
                onChange={(event) => setToken(event.target.value)}
                placeholder={t("auth.tokenPlaceholder")}
              />
            </label>
            <Button variant="secondary" onClick={saveToken}>
              {t("auth.enterWithToken")}
            </Button>
          </div>
        </details>
        {message ? <p className="form-message">{message}</p> : null}
        {oidcConfig.error ? (
          <p className="form-error">
            {oidcConfig.error instanceof Error
              ? oidcConfig.error.message
              : t("auth.oidcLoadFailed")}
          </p>
        ) : null}
      </section>
    </div>
  );
}

function extractAccessToken(input: string): string {
  const value = input.trim();
  if (!value) {
    return "";
  }
  if (!value.startsWith("{")) {
    return value;
  }
  try {
    const parsed = JSON.parse(value) as { access_token?: unknown; accessToken?: unknown };
    if (typeof parsed.access_token === "string") {
      return parsed.access_token.trim();
    }
    if (typeof parsed.accessToken === "string") {
      return parsed.accessToken.trim();
    }
  } catch {
    return value;
  }
  return value;
}
