import { createMachine } from "xstate";

export const approvalMachine = createMachine({
  id: "approval",
  initial: "idle",
  states: {
    idle: { on: { LOAD: "loadingPending" } },
    loadingPending: {
      on: {
        LOADED: "pending",
        FAIL: "failed"
      }
    },
    pending: {
      on: {
        DECIDE: "deciding",
        REFRESH: "loadingPending"
      }
    },
    deciding: {
      on: {
        DECIDED: "decided",
        CONFLICT: "conflict",
        FAIL: "failed"
      }
    },
    decided: { on: { LOAD: "loadingPending" } },
    conflict: { on: { LOAD: "loadingPending" } },
    failed: { on: { RETRY: "loadingPending" } }
  }
});
