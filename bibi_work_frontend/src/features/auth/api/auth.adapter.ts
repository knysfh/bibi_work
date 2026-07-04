import {
  genericResponseDtoSchema,
  mapMe,
  mapOidcConfig,
  meDtoSchema,
  oidcConfigDtoSchema,
  type Me,
  type OidcConfig
} from "../../../shared/contracts/platform";
import type { HttpClient } from "../../../shared/api/http-client";

export interface AuthApi {
  getOidcConfig(): Promise<OidcConfig>;
  getMe(): Promise<Me>;
  logout(): Promise<void>;
}

export function createAuthApi(http: HttpClient): AuthApi {
  return {
    async getOidcConfig() {
      return mapOidcConfig(
        await http.get("/auth/oidc/config", oidcConfigDtoSchema, { auth: false })
      );
    },
    async getMe() {
      return mapMe(await http.get("/me", meDtoSchema));
    },
    async logout() {
      await http.post("/auth/logout", {}, genericResponseDtoSchema);
    }
  };
}
