import type { JsonValue } from "../../../shared/types/json";
import {
  dtoList,
  mapRun,
  mapRunEvent,
  runDtoSchema,
  streamEventDtoSchema,
  type Run,
  type RunEvent
} from "../../../shared/contracts/platform";
import { getSse, parseSseJson, postSse } from "../../../shared/api/sse-client";
import type { HttpClient } from "../../../shared/api/http-client";

export interface CreateRunInput {
  tenantId: string;
  conversationId: string;
  agentId?: string;
  agentVersionId?: string;
  projectId?: string;
  input?: JsonValue;
  runConfigSnapshot?: JsonValue;
  idempotencyKey?: string;
  threadId?: string;
  afterSeq?: number;
}

export interface RunApi {
  listRuns(tenantId: string, filters?: { status?: string; limit?: number }): Promise<Run[]>;
  getRun(runId: string): Promise<Run>;
  cancelRun(runId: string): Promise<Run>;
  listConversationEvents(
    tenantId: string,
    conversationId: string,
    afterSeq?: number
  ): Promise<RunEvent[]>;
  createRunStream(
    input: CreateRunInput,
    onEvent: (event: RunEvent) => void,
    signal?: AbortSignal
  ): Promise<void>;
  subscribeConversationEvents(
    input: { tenantId: string; conversationId: string; afterSeq?: number },
    onEvent: (event: RunEvent) => void,
    signal?: AbortSignal
  ): Promise<void>;
}

export function createRunApi(http: HttpClient): RunApi {
  return {
    async listRuns(tenantId, filters = {}) {
      return (
        await http.get("/runs", dtoList(runDtoSchema), {
          query: { tenant_id: tenantId, status: filters.status, limit: filters.limit ?? 100 }
        })
      ).map(mapRun);
    },
    async getRun(runId) {
      return mapRun(await http.get(`/runs/${runId}`, runDtoSchema));
    },
    async cancelRun(runId) {
      return mapRun(await http.post(`/runs/${runId}/cancel`, {}, runDtoSchema));
    },
    async listConversationEvents(tenantId, conversationId, afterSeq) {
      return (
        await http.get(`/conversations/${conversationId}/events`, dtoList(streamEventDtoSchema), {
          query: { tenant_id: tenantId, after_seq: afterSeq }
        })
      ).map(mapRunEvent);
    },
    async createRunStream(input, onEvent, signal) {
      await postSse(
        http,
        `/conversations/${input.conversationId}/runs:stream`,
        {
          tenant_id: input.tenantId,
          agent_id: input.agentId,
          agent_version_id: input.agentVersionId,
          project_id: input.projectId,
          input: input.input,
          run_config_snapshot: input.runConfigSnapshot,
          idempotency_key: input.idempotencyKey,
          thread_id: input.threadId
        },
        (message) => {
          const payload = parseSseJson(message);
          if (!payload) {
            return;
          }
          onEvent(mapRunEvent(streamEventDtoSchema.parse(payload)));
        },
        signal,
        { tenant_id: input.tenantId, after_seq: input.afterSeq }
      );
    },
    async subscribeConversationEvents(input, onEvent, signal) {
      await getSse(
        http,
        `/conversations/${input.conversationId}/events/stream`,
        (message) => {
          if (message.event?.startsWith("stream.")) {
            return;
          }
          const payload = parseSseJson(message);
          if (!payload) {
            return;
          }
          onEvent(mapRunEvent(streamEventDtoSchema.parse(payload)));
        },
        signal,
        { tenant_id: input.tenantId, after_seq: input.afterSeq }
      );
    }
  };
}
