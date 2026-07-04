import type { JsonValue } from "../../../shared/types/json";
import {
  conversationDtoSchema,
  dtoList,
  mapConversation,
  type Conversation
} from "../../../shared/contracts/platform";
import type { HttpClient } from "../../../shared/api/http-client";

export interface CreateConversationInput {
  tenantId: string;
  title?: string;
  workspaceId?: string;
  projectId?: string;
  agentId?: string;
  metadata?: JsonValue;
}

export interface ConversationApi {
  listConversations(tenantId: string, limit?: number): Promise<Conversation[]>;
  createConversation(input: CreateConversationInput): Promise<Conversation>;
}

export function createConversationApi(http: HttpClient): ConversationApi {
  return {
    async listConversations(tenantId, limit = 100) {
      return (
        await http.get("/conversations", dtoList(conversationDtoSchema), {
          query: { tenant_id: tenantId, limit }
        })
      ).map(mapConversation);
    },
    async createConversation(input) {
      return mapConversation(
        await http.post(
          "/conversations",
          {
            tenant_id: input.tenantId,
            title: input.title,
            workspace_id: input.workspaceId,
            project_id: input.projectId,
            agent_id: input.agentId,
            metadata: input.metadata
          },
          conversationDtoSchema
        )
      );
    }
  };
}
