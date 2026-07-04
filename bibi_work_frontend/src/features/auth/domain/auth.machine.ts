import { createMachine } from "xstate";

export const appBootstrapMachine = createMachine({
  id: "appBootstrap",
  initial: "booting",
  states: {
    booting: { on: { LOAD_TOKEN: "loadingStoredToken" } },
    loadingStoredToken: {
      on: {
        TOKEN_FOUND: "refreshingToken",
        TOKEN_MISSING: "unauthenticated",
        FAIL: "degraded"
      }
    },
    unauthenticated: {
      on: { LOGIN_STARTED: "refreshingToken" }
    },
    refreshingToken: {
      on: {
        TOKEN_READY: "loadingMe",
        TOKEN_EXPIRED: "unauthenticated",
        FAIL: "degraded"
      }
    },
    loadingMe: {
      on: {
        ME_LOADED: "ready",
        SESSION_REVOKED: "unauthenticated",
        FAIL: "degraded"
      }
    },
    ready: {
      on: {
        LOGOUT: "unauthenticated",
        SESSION_REVOKED: "unauthenticated",
        FAIL: "degraded"
      }
    },
    degraded: {
      on: {
        RETRY: "loadingStoredToken",
        LOGOUT: "unauthenticated"
      }
    },
    fatal: {}
  }
});

export const oidcLoginMachine = createMachine({
  id: "oidcLogin",
  initial: "idle",
  states: {
    idle: { on: { START: "preparingPkce" } },
    preparingPkce: { on: { PKCE_READY: "openingBrowser", FAIL: "failed" } },
    openingBrowser: { on: { BROWSER_OPENED: "waitingCallback", FAIL: "failed" } },
    waitingCallback: {
      on: {
        CALLBACK_RECEIVED: "exchangingCode",
        CANCEL: "idle",
        FAIL: "failed"
      }
    },
    exchangingCode: { on: { TOKEN_RECEIVED: "savingToken", FAIL: "failed" } },
    savingToken: { on: { TOKEN_SAVED: "loadingMe", FAIL: "failed" } },
    loadingMe: { on: { ME_LOADED: "authenticated", FAIL: "failed" } },
    authenticated: { on: { LOGOUT: "idle" } },
    failed: { on: { RETRY: "preparingPkce", CANCEL: "idle" } }
  }
});
