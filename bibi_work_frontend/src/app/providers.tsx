import { QueryClientProvider } from "@tanstack/react-query";
import { createContext, useContext, useMemo, type PropsWithChildren } from "react";
import { createApprovalApi, type ApprovalApi } from "../features/approvals/api/approval.adapter";
import { createAuthApi, type AuthApi } from "../features/auth/api/auth.adapter";
import { createCatalogApi, type CatalogApi } from "../features/catalog/api/catalog.adapter";
import {
  createConversationApi,
  type ConversationApi
} from "../features/conversations/api/conversation.adapter";
import { createMemoryApi, type MemoryApi } from "../features/memories/api/memory.adapter";
import { createLlmApi, type LlmApi } from "../features/llm/api/llm.adapter";
import { createProjectApi, type ProjectApi } from "../features/projects/api/project.adapter";
import { createRunApi, type RunApi } from "../features/runs/api/run.adapter";
import {
  createSecretRefsApi,
  type SecretRefsApi
} from "../features/secrets/api/secret-refs.adapter";
import {
  createSessionDeviceApi,
  type SessionDeviceApi
} from "../features/session-device/api/session-device.adapter";
import {
  createWorkspaceApi,
  type WorkspaceApi
} from "../features/workspaces/api/workspace.adapter";
import { createHttpClient, type HttpClient } from "../shared/api/http-client";
import { createQueryClient } from "../shared/api/query-client";
import type { TokenProvider, TokenSet } from "../shared/api/token-provider";
import { I18nProvider } from "../shared/i18n";
import { createDesktopAuthApi, type DesktopAuthApi } from "../shared/tauri/desktop-auth";
import { AppEventBus } from "../shared/tauri/event-bus";
import { createInvokeClient } from "../shared/tauri/invoke-client";

export interface PlatformApiContextValue {
  apiBaseUrl: string;
  http: HttpClient;
  tokenProvider: TokenProvider;
  desktopAuthApi: DesktopAuthApi;
  eventBus: AppEventBus;
  authApi: AuthApi;
  sessionDeviceApi: SessionDeviceApi;
  workspaceApi: WorkspaceApi;
  conversationApi: ConversationApi;
  runApi: RunApi;
  approvalApi: ApprovalApi;
  projectApi: ProjectApi;
  memoryApi: MemoryApi;
  llmApi: LlmApi;
  catalogApi: CatalogApi;
  secretRefsApi: SecretRefsApi;
}

const PlatformApiContext = createContext<PlatformApiContextValue | null>(null);

export function PlatformProviders({ children }: PropsWithChildren) {
  const queryClient = useMemo(() => createQueryClient(), []);
  const platformApi = useMemo(() => createPlatformApi(), []);

  return (
    <PlatformApiContext.Provider value={platformApi}>
      <I18nProvider>
        <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
      </I18nProvider>
    </PlatformApiContext.Provider>
  );
}

export function usePlatformApi(): PlatformApiContextValue {
  const value = useContext(PlatformApiContext);
  if (!value) {
    throw new Error("usePlatformApi must be used within PlatformProviders");
  }
  return value;
}

function createPlatformApi(): PlatformApiContextValue {
  const apiBaseUrl =
    import.meta.env.VITE_BIBI_WORK_API_BASE_URL?.replace(/\/+$/, "") ??
    "http://localhost:8361/api/v1";
  const desktopAuthApi = createDesktopAuthApi(createInvokeClient());
  let cachedTokenSet: TokenSet | null = null;
  const tokenProvider: TokenProvider = {
    async getAccessToken() {
      if (cachedTokenSet?.accessToken) {
        return cachedTokenSet.accessToken;
      }
      cachedTokenSet = await desktopAuthApi.loadTokenSet();
      return cachedTokenSet?.accessToken ?? null;
    },
    async setTokenSet(tokenSet: TokenSet) {
      cachedTokenSet = tokenSet;
      await desktopAuthApi.saveTokenSet(tokenSet);
    },
    async clearTokenSet() {
      cachedTokenSet = null;
      await desktopAuthApi.clearTokenSet();
    }
  };
  const http = createHttpClient({ baseUrl: apiBaseUrl, tokenProvider });
  const eventBus = new AppEventBus();

  return {
    apiBaseUrl,
    http,
    tokenProvider,
    desktopAuthApi,
    eventBus,
    authApi: createAuthApi(http),
    sessionDeviceApi: createSessionDeviceApi(http),
    workspaceApi: createWorkspaceApi(http),
    conversationApi: createConversationApi(http),
    runApi: createRunApi(http),
    approvalApi: createApprovalApi(http),
    projectApi: createProjectApi(http),
    memoryApi: createMemoryApi(http),
    llmApi: createLlmApi(http),
    catalogApi: createCatalogApi(http),
    secretRefsApi: createSecretRefsApi(http)
  };
}
