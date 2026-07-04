import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { usePlatformApi } from "../../../app/providers";
import type {
  CreateLlmCredentialInput,
  CreateLlmModelProfileInput,
  CreateLlmProviderInput,
  DisableLlmResourceInput,
  LlmCredentialListInput,
  LlmListInput,
  RevokeLlmCredentialInput,
  TestLlmProfileInput,
  UpdateLlmModelProfileInput,
  UpdateLlmProviderInput
} from "./llm.adapter";

export const llmQueryKeys = {
  providers: (input: LlmListInput) => [
    "llmProviders",
    input.tenantId,
    input.status ?? "all",
    input.limit ?? 100
  ],
  credentials: (input: LlmCredentialListInput) => [
    "llmCredentials",
    input.tenantId,
    input.providerId ?? "all",
    input.status ?? "all",
    input.limit ?? 100
  ],
  profiles: (input: LlmListInput) => [
    "llmModelProfiles",
    input.tenantId,
    input.status ?? "all",
    input.limit ?? 100
  ]
};

export function useLlmProvidersQuery(input: LlmListInput) {
  const { llmApi } = usePlatformApi();
  return useQuery({
    queryKey: llmQueryKeys.providers(input),
    queryFn: () => llmApi.listProviders(input),
    enabled: Boolean(input.tenantId)
  });
}

export function useLlmCredentialsQuery(input: LlmCredentialListInput) {
  const { llmApi } = usePlatformApi();
  return useQuery({
    queryKey: llmQueryKeys.credentials(input),
    queryFn: () => llmApi.listCredentials(input),
    enabled: Boolean(input.tenantId)
  });
}

export function useLlmProfilesQuery(input: LlmListInput) {
  const { llmApi } = usePlatformApi();
  return useQuery({
    queryKey: llmQueryKeys.profiles(input),
    queryFn: () => llmApi.listProfiles(input),
    enabled: Boolean(input.tenantId)
  });
}

export function useCreateLlmProviderMutation() {
  const { llmApi } = usePlatformApi();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (input: CreateLlmProviderInput) => llmApi.createProvider(input),
    onSuccess: (_provider, input) => invalidateProviders(queryClient, input.tenantId)
  });
}

export function useUpdateLlmProviderMutation() {
  const { llmApi } = usePlatformApi();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (input: UpdateLlmProviderInput) => llmApi.updateProvider(input),
    onSuccess: (_provider, input) => invalidateProviders(queryClient, input.tenantId)
  });
}

export function useDisableLlmProviderMutation() {
  const { llmApi } = usePlatformApi();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (input: DisableLlmResourceInput) => llmApi.disableProvider(input),
    onSuccess: (_provider, input) => invalidateProviders(queryClient, input.tenantId)
  });
}

export function useCreateLlmCredentialMutation() {
  const { llmApi } = usePlatformApi();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (input: CreateLlmCredentialInput) => llmApi.createCredential(input),
    onSuccess: (_credential, input) => invalidateCredentials(queryClient, input.tenantId)
  });
}

export function useRevokeLlmCredentialMutation() {
  const { llmApi } = usePlatformApi();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (input: RevokeLlmCredentialInput) => llmApi.revokeCredential(input),
    onSuccess: (_credential, input) => invalidateCredentials(queryClient, input.tenantId)
  });
}

export function useCreateLlmProfileMutation() {
  const { llmApi } = usePlatformApi();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (input: CreateLlmModelProfileInput) => llmApi.createProfile(input),
    onSuccess: (_profile, input) => invalidateProfiles(queryClient, input.tenantId)
  });
}

export function useUpdateLlmProfileMutation() {
  const { llmApi } = usePlatformApi();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (input: UpdateLlmModelProfileInput) => llmApi.updateProfile(input),
    onSuccess: (_profile, input) => invalidateProfiles(queryClient, input.tenantId)
  });
}

export function useDisableLlmProfileMutation() {
  const { llmApi } = usePlatformApi();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (input: DisableLlmResourceInput) => llmApi.disableProfile(input),
    onSuccess: (_profile, input) => invalidateProfiles(queryClient, input.tenantId)
  });
}

export function useTestLlmProfileMutation() {
  const { llmApi } = usePlatformApi();
  return useMutation({
    mutationFn: (input: TestLlmProfileInput) => llmApi.testProfile(input)
  });
}

function invalidateProviders(queryClient: ReturnType<typeof useQueryClient>, tenantId: string) {
  return queryClient.invalidateQueries({ queryKey: ["llmProviders", tenantId] });
}

function invalidateCredentials(queryClient: ReturnType<typeof useQueryClient>, tenantId: string) {
  return queryClient.invalidateQueries({ queryKey: ["llmCredentials", tenantId] });
}

function invalidateProfiles(queryClient: ReturnType<typeof useQueryClient>, tenantId: string) {
  return queryClient.invalidateQueries({ queryKey: ["llmModelProfiles", tenantId] });
}
