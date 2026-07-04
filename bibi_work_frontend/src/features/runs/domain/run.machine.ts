import { createMachine } from "xstate";

export const runStreamMachine = createMachine({
  id: "runStream",
  initial: "idle",
  states: {
    idle: { on: { CREATE: "creatingRun" } },
    creatingRun: {
      on: {
        RUN_CREATED: "connectingStream",
        FAIL: "failed"
      }
    },
    connectingStream: {
      on: {
        CONNECTED: "streaming",
        RECONNECT: "reconnecting",
        FAIL: "failed"
      }
    },
    streaming: {
      on: {
        APPROVAL_REQUESTED: "waitingApproval",
        USER_INPUT_REQUESTED: "waitingUserInput",
        CANCEL: "cancelling",
        RECONNECT: "reconnecting",
        COMPLETE: "completed",
        FAIL: "failed"
      }
    },
    waitingApproval: {
      on: {
        APPROVAL_COMPLETED: "streaming",
        CANCEL: "cancelling",
        FAIL: "failed"
      }
    },
    waitingUserInput: {
      on: {
        USER_INPUT_SENT: "streaming",
        CANCEL: "cancelling",
        FAIL: "failed"
      }
    },
    cancelling: {
      on: {
        CANCELLED: "cancelled",
        FAIL: "failed"
      }
    },
    reconnecting: {
      on: {
        CONNECTED: "streaming",
        FAIL: "failed"
      }
    },
    completed: { type: "final" },
    failed: {},
    cancelled: { type: "final" }
  }
});
