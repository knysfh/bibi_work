import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { usePlatformApi } from "../../../app/providers";

export const authQueryKeys = {
  oidcConfig: ["auth", "oidcConfig"] as const,
  me: ["me"] as const
};

export function useOidcConfigQuery() {
  const { authApi } = usePlatformApi();
  return useQuery({
    queryKey: authQueryKeys.oidcConfig,
    queryFn: () => authApi.getOidcConfig()
  });
}

export function useMeQuery(enabled = true) {
  const { authApi } = usePlatformApi();
  return useQuery({
    queryKey: authQueryKeys.me,
    queryFn: () => authApi.getMe(),
    enabled
  });
}

export function useLogoutMutation(onLocalLogout: () => Promise<void>) {
  const { authApi } = usePlatformApi();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async () => {
      await authApi.logout();
      await onLocalLogout();
    },
    onSettled: async () => {
      queryClient.clear();
    }
  });
}
