import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { usePlatformApi } from "../../../app/providers";
import type { CreateConversationInput } from "./conversation.adapter";

export const conversationQueryKeys = {
  list: (tenantId: string) => ["conversations", tenantId] as const
};

export function useConversationsQuery(tenantId?: string) {
  const { conversationApi } = usePlatformApi();
  return useQuery({
    queryKey: conversationQueryKeys.list(tenantId ?? ""),
    queryFn: () => conversationApi.listConversations(tenantId ?? ""),
    enabled: Boolean(tenantId)
  });
}

export function useCreateConversationMutation(tenantId: string) {
  const { conversationApi } = usePlatformApi();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (input: Omit<CreateConversationInput, "tenantId">) =>
      conversationApi.createConversation({ ...input, tenantId }),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: conversationQueryKeys.list(tenantId) });
    }
  });
}
