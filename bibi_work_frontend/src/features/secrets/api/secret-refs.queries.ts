import { useQuery } from "@tanstack/react-query";
import { usePlatformApi } from "../../../app/providers";
import type { SecretRefListInput } from "./secret-refs.adapter";

export const secretRefQueryKeys = {
  list: (input: SecretRefListInput) => [
    "secretRefs",
    input.tenantId,
    input.purpose ?? "all"
  ]
};

export function useSecretRefsQuery(input: SecretRefListInput) {
  const { secretRefsApi } = usePlatformApi();
  return useQuery({
    queryKey: secretRefQueryKeys.list(input),
    queryFn: () => secretRefsApi.listSecretRefs(input),
    enabled: Boolean(input.tenantId)
  });
}
