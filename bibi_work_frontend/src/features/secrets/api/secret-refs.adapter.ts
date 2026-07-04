import { z } from "zod";
import type { HttpClient } from "../../../shared/api/http-client";

export interface SecretRefListInput {
  tenantId: string;
  purpose?: string;
}

export interface SecretRef {
  id: string;
  label: string;
  purpose: string;
  scheme: string;
  available: boolean;
}

export interface SecretRefsApi {
  listSecretRefs(input: SecretRefListInput): Promise<SecretRef[]>;
}

const secretRefDtoSchema = z.object({
  id: z.string(),
  label: z.string(),
  purpose: z.string(),
  scheme: z.string(),
  available: z.boolean()
});

export function createSecretRefsApi(http: HttpClient): SecretRefsApi {
  return {
    listSecretRefs(input) {
      return http.get("/secret-refs", z.array(secretRefDtoSchema), {
        query: {
          tenant_id: input.tenantId,
          purpose: input.purpose
        }
      });
    }
  };
}
