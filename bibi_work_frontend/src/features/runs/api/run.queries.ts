import { useMutation, useQuery, useQueryClient, type QueryClient } from "@tanstack/react-query";
import { useEffect, useRef, useState } from "react";
import { usePlatformApi } from "../../../app/providers";
import type { RunEvent } from "../../../shared/contracts/platform";
import { mergeRunEvents } from "../domain/run.projections";

export const runQueryKeys = {
  list: (tenantId: string, status?: string) => ["runs", tenantId, status ?? "all"] as const,
  detail: (runId: string) => ["run", runId] as const,
  events: (tenantId: string, conversationId: string) =>
    ["conversationEvents", tenantId, conversationId] as const
};

export function useRunsQuery(tenantId?: string, status?: string) {
  const { runApi } = usePlatformApi();
  return useQuery({
    queryKey: runQueryKeys.list(tenantId ?? "", status),
    queryFn: () => runApi.listRuns(tenantId ?? "", { status }),
    enabled: Boolean(tenantId)
  });
}

export function useConversationEventsQuery(tenantId?: string, conversationId?: string) {
  const { runApi } = usePlatformApi();
  const queryClient = useQueryClient();
  const queryKey = runQueryKeys.events(tenantId ?? "", conversationId ?? "");
  return useQuery({
    queryKey,
    queryFn: async () => {
      const afterSeq = latestCachedSeq(queryClient, tenantId ?? "", conversationId ?? "");
      const fetched = await runApi.listConversationEvents(
        tenantId ?? "",
        conversationId ?? "",
        afterSeq
      );
      const current = queryClient.getQueryData<RunEvent[]>(queryKey) ?? [];
      return mergeRunEvents(current, fetched);
    },
    enabled: Boolean(tenantId && conversationId)
  });
}

export type ConversationEventStreamStatus = "idle" | "connecting" | "connected" | "reconnecting";

export interface ConversationEventStreamState {
  status: ConversationEventStreamStatus;
  lastError?: string;
}

export function patchConversationEvents(
  queryClient: QueryClient,
  tenantId: string,
  conversationId: string,
  events: RunEvent[]
) {
  if (!events.length) {
    return;
  }
  queryClient.setQueryData<RunEvent[]>(
    runQueryKeys.events(tenantId, conversationId),
    (current = []) => mergeRunEvents(current, events)
  );
}

export function useConversationEventStream(
  tenantId?: string,
  conversationId?: string,
  onEvent?: (event: RunEvent) => void
): ConversationEventStreamState {
  const { runApi } = usePlatformApi();
  const queryClient = useQueryClient();
  const onEventRef = useRef(onEvent);
  const [state, setState] = useState<ConversationEventStreamState>({ status: "idle" });

  useEffect(() => {
    onEventRef.current = onEvent;
  }, [onEvent]);

  useEffect(() => {
    if (!tenantId || !conversationId) {
      setState({ status: "idle" });
      return;
    }

    const streamTenantId = tenantId;
    const streamConversationId = conversationId;
    let stopped = false;
    const controller = new AbortController();

    async function connect() {
      let reconnectCount = 0;
      while (!stopped && !controller.signal.aborted) {
        const afterSeq = latestCachedSeq(queryClient, streamTenantId, streamConversationId);
        setState({
          status: reconnectCount === 0 ? "connecting" : "reconnecting"
        });
        try {
          await runApi.subscribeConversationEvents(
            { tenantId: streamTenantId, conversationId: streamConversationId, afterSeq },
            (event) => {
              patchConversationEvents(queryClient, event.tenantId, event.conversationId, [event]);
              onEventRef.current?.(event);
              setState({ status: "connected" });
            },
            controller.signal
          );
          reconnectCount += 1;
          await waitForReconnect(reconnectDelayMs(reconnectCount), controller.signal);
        } catch (error) {
          if (stopped || controller.signal.aborted || isAbortError(error)) {
            return;
          }
          reconnectCount += 1;
          setState({
            status: "reconnecting",
            lastError: error instanceof Error ? error.message : String(error)
          });
          await waitForReconnect(reconnectDelayMs(reconnectCount), controller.signal);
        }
      }
    }

    void connect();

    return () => {
      stopped = true;
      controller.abort();
    };
  }, [conversationId, queryClient, runApi, tenantId]);

  return state;
}

export function useCancelRunMutation(tenantId: string) {
  const { runApi } = usePlatformApi();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (runId: string) => runApi.cancelRun(runId),
    onSuccess: async (run) => {
      await queryClient.invalidateQueries({ queryKey: runQueryKeys.detail(run.id) });
      await queryClient.invalidateQueries({ queryKey: runQueryKeys.list(tenantId) });
      await queryClient.invalidateQueries({
        queryKey: runQueryKeys.events(tenantId, run.conversationId)
      });
    }
  });
}

function latestCachedSeq(
  queryClient: QueryClient,
  tenantId: string,
  conversationId: string
): number {
  return latestSeq(
    queryClient.getQueryData<RunEvent[]>(runQueryKeys.events(tenantId, conversationId))
  );
}

function latestSeq(events: RunEvent[] | undefined): number {
  return (events ?? []).reduce((maxSeq, event) => Math.max(maxSeq, event.seq), 0);
}

function reconnectDelayMs(reconnectCount: number): number {
  return Math.min(5000, reconnectCount * 1000);
}

function waitForReconnect(delayMs: number, signal: AbortSignal): Promise<void> {
  return new Promise((resolve) => {
    if (signal.aborted) {
      resolve();
      return;
    }
    const timeoutId = window.setTimeout(resolve, delayMs);
    signal.addEventListener(
      "abort",
      () => {
        window.clearTimeout(timeoutId);
        resolve();
      },
      { once: true }
    );
  });
}

function isAbortError(error: unknown): boolean {
  return error instanceof DOMException && error.name === "AbortError";
}
