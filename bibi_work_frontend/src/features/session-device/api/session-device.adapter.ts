import {
  dtoList,
  deviceDtoSchema,
  mapDevice,
  mapSession,
  sessionDtoSchema
} from "../../../shared/contracts/platform";
import type { Device, Session } from "../../../shared/contracts/platform";
import type { HttpClient } from "../../../shared/api/http-client";

export interface SessionDeviceApi {
  listDevices(tenantId: string, limit?: number): Promise<Device[]>;
  listSessions(tenantId: string, limit?: number): Promise<Session[]>;
  revokeDevice(tenantId: string, deviceId: string): Promise<Device>;
  revokeSession(tenantId: string, sessionId: string): Promise<Session>;
}

export function createSessionDeviceApi(http: HttpClient): SessionDeviceApi {
  return {
    async listDevices(tenantId, limit = 100) {
      return (
        await http.get("/devices", dtoList(deviceDtoSchema), {
          query: { tenant_id: tenantId, limit }
        })
      ).map(mapDevice);
    },
    async listSessions(tenantId, limit = 100) {
      return (
        await http.get("/sessions", dtoList(sessionDtoSchema), {
          query: { tenant_id: tenantId, limit }
        })
      ).map(mapSession);
    },
    async revokeDevice(tenantId, deviceId) {
      return mapDevice(
        await http.post(`/devices/${deviceId}/revoke`, { tenant_id: tenantId }, deviceDtoSchema)
      );
    },
    async revokeSession(tenantId, sessionId) {
      return mapSession(
        await http.post(`/sessions/${sessionId}/revoke`, { tenant_id: tenantId }, sessionDtoSchema)
      );
    }
  };
}
