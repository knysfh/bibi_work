import {
  dtoList,
  mapMemoryBatchDecisionResponse,
  mapMemoryItem,
  memoryBatchDecisionResponseDtoSchema,
  memoryItemDtoSchema,
  type MemoryBatchDecisionResponse,
  type MemoryItem
} from "../../../shared/contracts/platform";
import type { HttpClient } from "../../../shared/api/http-client";

export type MemoryLayer = "core_profile" | "episodic" | "semantic" | "procedural";
export type MemoryStatus = "candidate" | "approved" | "rejected" | "archived";
export type MemoryVisibility = "private" | "tenant" | "public";
export type MemorySensitivity = "normal" | "sensitive" | "secret";
export type MemoryDecision = "activate" | "reject" | "archive";

export interface MemoryFilters {
  tenantId: string;
  userId?: string;
  agentId?: string;
  projectId?: string;
  layer?: MemoryLayer;
  status?: MemoryStatus;
  query?: string;
  runId?: string;
  limit?: number;
}

export interface CreateMemoryInput {
  tenantId: string;
  userId?: string;
  agentId?: string;
  projectId?: string;
  layer: MemoryLayer;
  content: string;
  sourceRunId?: string;
  confidence?: number;
  status?: MemoryStatus;
  visibility?: MemoryVisibility;
  retentionPolicy?: string;
  sensitivity?: MemorySensitivity;
}

export interface MemoryApi {
  listMemories(filters: MemoryFilters): Promise<MemoryItem[]>;
  searchMemories(filters: MemoryFilters): Promise<MemoryItem[]>;
  createMemory(input: CreateMemoryInput): Promise<MemoryItem>;
  decideMemory(memoryId: string, decision: MemoryDecision): Promise<MemoryItem>;
  batchDecideMemories(input: {
    tenantId: string;
    decision: MemoryDecision;
    memoryIds: string[];
    runId?: string;
  }): Promise<MemoryBatchDecisionResponse>;
}

export function createMemoryApi(http: HttpClient): MemoryApi {
  return {
    async listMemories(filters) {
      return (
        await http.get("/memories", dtoList(memoryItemDtoSchema), {
          query: toMemoryQuery(filters)
        })
      ).map(mapMemoryItem);
    },
    async searchMemories(filters) {
      return (
        await http.post("/memories:search", toMemoryBody(filters), dtoList(memoryItemDtoSchema))
      ).map(mapMemoryItem);
    },
    async createMemory(input) {
      return mapMemoryItem(
        await http.post(
          "/memories",
          {
            tenant_id: input.tenantId,
            user_id: input.userId,
            agent_id: input.agentId,
            project_id: input.projectId,
            layer: input.layer,
            content: input.content,
            source_run_id: input.sourceRunId,
            confidence: input.confidence,
            status: input.status,
            visibility: input.visibility,
            retention_policy: input.retentionPolicy,
            sensitivity: input.sensitivity
          },
          memoryItemDtoSchema
        )
      );
    },
    async decideMemory(memoryId, decision) {
      return mapMemoryItem(
        await http.post(`/memories/${memoryId}/${decision}`, {}, memoryItemDtoSchema)
      );
    },
    async batchDecideMemories(input) {
      return mapMemoryBatchDecisionResponse(
        await http.post(
          "/memories:batch-decision",
          {
            tenant_id: input.tenantId,
            decision: input.decision,
            run_id: input.runId,
            memory_ids: input.memoryIds
          },
          memoryBatchDecisionResponseDtoSchema
        )
      );
    }
  };
}

function toMemoryQuery(filters: MemoryFilters) {
  return {
    tenant_id: filters.tenantId,
    user_id: filters.userId,
    agent_id: filters.agentId,
    project_id: filters.projectId,
    layer: filters.layer,
    status: filters.status,
    query: filters.query,
    run_id: filters.runId,
    limit: filters.limit
  };
}

function toMemoryBody(filters: MemoryFilters) {
  return {
    tenant_id: filters.tenantId,
    user_id: filters.userId,
    agent_id: filters.agentId,
    project_id: filters.projectId,
    layer: filters.layer,
    status: filters.status,
    query: filters.query,
    run_id: filters.runId,
    limit: filters.limit
  };
}
