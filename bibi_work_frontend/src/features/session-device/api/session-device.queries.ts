import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { usePlatformApi } from "../../../app/providers";

export const sessionDeviceQueryKeys = {
  devices: (tenantId: string) => ["devices", tenantId] as const,
  sessions: (tenantId: string) => ["sessions", tenantId] as const
};

export function useDevicesQuery(tenantId?: string) {
  const { sessionDeviceApi } = usePlatformApi();
  return useQuery({
    queryKey: sessionDeviceQueryKeys.devices(tenantId ?? ""),
    queryFn: () => sessionDeviceApi.listDevices(tenantId ?? ""),
    enabled: Boolean(tenantId)
  });
}

export function useSessionsQuery(tenantId?: string) {
  const { sessionDeviceApi } = usePlatformApi();
  return useQuery({
    queryKey: sessionDeviceQueryKeys.sessions(tenantId ?? ""),
    queryFn: () => sessionDeviceApi.listSessions(tenantId ?? ""),
    enabled: Boolean(tenantId)
  });
}

export function useRevokeSessionMutation(tenantId: string) {
  const { sessionDeviceApi } = usePlatformApi();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (sessionId: string) => sessionDeviceApi.revokeSession(tenantId, sessionId),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: sessionDeviceQueryKeys.sessions(tenantId) });
    }
  });
}

export function useRevokeDeviceMutation(tenantId: string) {
  const { sessionDeviceApi } = usePlatformApi();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (deviceId: string) => sessionDeviceApi.revokeDevice(tenantId, deviceId),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: sessionDeviceQueryKeys.devices(tenantId) });
      await queryClient.invalidateQueries({ queryKey: sessionDeviceQueryKeys.sessions(tenantId) });
    }
  });
}
