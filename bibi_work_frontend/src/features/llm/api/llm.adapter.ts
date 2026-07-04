import {
  dtoList,
  mapResource,
  resourceDtoSchema,
  type Resource
} from "../../../shared/contracts/platform";
import type { HttpClient } from "../../../shared/api/http-client";
import { asRecord, stringFromJson, type JsonValue } from "../../../shared/types/json";
import { z } from "zod";

export interface LlmListInput {
  tenantId: string;
  status?: string;
  limit?: number;
}

export interface LlmCredentialListInput extends LlmListInput {
  providerId?: string;
}

export interface LlmProvider {
  id: string;
  tenantId: string;
  providerKey: string;
  displayName: string;
  baseUrl?: string;
  authScheme: string;
  defaultHeadersTemplate: JsonValue;
  status: string;
  createdAt: string;
  updatedAt?: string;
  raw: Resource;
}

export interface LlmCredential {
  id: string;
  tenantId: string;
  label: string;
  providerId: string;
  providerKey?: string;
  providerName?: string;
  ownerScope: string;
  ownerResourceId?: string;
  hasSecretRef: boolean;
  hasSecretHash: boolean;
  expiresAt?: string;
  revokedAt?: string;
  status: string;
  createdAt: string;
  updatedAt?: string;
  raw: Resource;
}

export interface LlmModelProfile {
  id: string;
  tenantId: string;
  profileName: string;
  modelName: string;
  providerId: string;
  credentialId?: string;
  contextWindow?: number;
  maxInputTokens?: number;
  maxOutputTokens?: number;
  temperature?: number;
  topP?: number;
  reasoningEffort?: string;
  responseFormat: JsonValue;
  toolChoicePolicy: JsonValue;
  rateLimitPolicy: JsonValue;
  costPolicy: JsonValue;
  status: string;
  createdAt: string;
  updatedAt?: string;
  raw: Resource;
}

export interface CreateLlmProviderInput {
  tenantId: string;
  providerKey: string;
  displayName: string;
  baseUrl?: string;
  authScheme?: string;
  defaultHeadersTemplate?: JsonValue;
}

export interface UpdateLlmProviderInput {
  tenantId: string;
  providerId: string;
  displayName?: string;
  baseUrl?: string;
  authScheme?: string;
  defaultHeadersTemplate?: JsonValue;
}

export interface CreateLlmCredentialInput {
  tenantId: string;
  providerId: string;
  ownerScope?: string;
  ownerResourceId?: string;
  secretRef: string;
  secretHash?: string;
  expiresAt?: string;
}

export interface RevokeLlmCredentialInput {
  tenantId: string;
  credentialId: string;
}

export interface CreateLlmModelProfileInput {
  tenantId: string;
  providerId: string;
  credentialId?: string;
  profileName: string;
  modelName: string;
  contextWindow?: number;
  maxInputTokens?: number;
  maxOutputTokens?: number;
  temperature?: number;
  topP?: number;
  reasoningEffort?: string;
  responseFormat?: JsonValue;
  toolChoicePolicy?: JsonValue;
  rateLimitPolicy?: JsonValue;
  costPolicy?: JsonValue;
}

export interface UpdateLlmModelProfileInput {
  tenantId: string;
  profileId: string;
  credentialId?: string;
  profileName?: string;
  modelName?: string;
  contextWindow?: number;
  maxInputTokens?: number;
  maxOutputTokens?: number;
  temperature?: number;
  topP?: number;
  reasoningEffort?: string;
  responseFormat?: JsonValue;
  toolChoicePolicy?: JsonValue;
  rateLimitPolicy?: JsonValue;
  costPolicy?: JsonValue;
}

export interface DisableLlmResourceInput {
  tenantId: string;
  resourceId: string;
}

export interface TestLlmProfileInput {
  tenantId: string;
  profileId: string;
}

export interface LlmProfileTestResult {
  success: boolean;
  providerKey: string;
  modelName: string;
  httpStatus?: number;
  latencyMs: number;
  message: string;
}

export interface LlmApi {
  listProviders(input: LlmListInput): Promise<LlmProvider[]>;
  createProvider(input: CreateLlmProviderInput): Promise<LlmProvider>;
  updateProvider(input: UpdateLlmProviderInput): Promise<LlmProvider>;
  disableProvider(input: DisableLlmResourceInput): Promise<LlmProvider>;
  listCredentials(input: LlmCredentialListInput): Promise<LlmCredential[]>;
  createCredential(input: CreateLlmCredentialInput): Promise<LlmCredential>;
  revokeCredential(input: RevokeLlmCredentialInput): Promise<LlmCredential>;
  listProfiles(input: LlmListInput): Promise<LlmModelProfile[]>;
  createProfile(input: CreateLlmModelProfileInput): Promise<LlmModelProfile>;
  updateProfile(input: UpdateLlmModelProfileInput): Promise<LlmModelProfile>;
  disableProfile(input: DisableLlmResourceInput): Promise<LlmModelProfile>;
  testProfile(input: TestLlmProfileInput): Promise<LlmProfileTestResult>;
}

const llmProfileTestResponseSchema = z.object({
  success: z.boolean(),
  provider_key: z.string(),
  model_name: z.string(),
  http_status: z.number().nullable().optional(),
  latency_ms: z.number(),
  message: z.string()
});

export function createLlmApi(http: HttpClient): LlmApi {
  return {
    async listProviders(input) {
      return (
        await http.get("/llm-providers", dtoList(resourceDtoSchema), {
          query: toListQuery(input)
        })
      )
        .map(mapResource)
        .map(mapLlmProvider);
    },
    async createProvider(input) {
      return mapLlmProvider(
        mapResource(await http.post("/llm-providers", toProviderBody(input), resourceDtoSchema))
      );
    },
    async updateProvider(input) {
      return mapLlmProvider(
        mapResource(
          await http.patch(
            `/llm-providers/${input.providerId}`,
            toProviderBody(input),
            resourceDtoSchema
          )
        )
      );
    },
    async disableProvider(input) {
      return mapLlmProvider(
        mapResource(
          await http.post(
            `/llm-providers/${input.resourceId}/disable`,
            { tenant_id: input.tenantId },
            resourceDtoSchema
          )
        )
      );
    },
    async listCredentials(input) {
      return (
        await http.get("/llm-credentials", dtoList(resourceDtoSchema), {
          query: { ...toListQuery(input), provider_id: input.providerId }
        })
      )
        .map(mapResource)
        .map(mapLlmCredential);
    },
    async createCredential(input) {
      return mapLlmCredential(
        mapResource(
          await http.post(
            "/llm-credentials",
            {
              tenant_id: input.tenantId,
              provider_id: input.providerId,
              owner_scope: input.ownerScope,
              owner_resource_id: input.ownerResourceId,
              secret_ref: input.secretRef,
              secret_hash: input.secretHash,
              expires_at: input.expiresAt
            },
            resourceDtoSchema
          )
        )
      );
    },
    async revokeCredential(input) {
      return mapLlmCredential(
        mapResource(
          await http.post(
            `/llm-credentials/${input.credentialId}/revoke`,
            { tenant_id: input.tenantId },
            resourceDtoSchema
          )
        )
      );
    },
    async listProfiles(input) {
      return (
        await http.get("/llm-model-profiles", dtoList(resourceDtoSchema), {
          query: toListQuery(input)
        })
      )
        .map(mapResource)
        .map(mapLlmModelProfile);
    },
    async createProfile(input) {
      return mapLlmModelProfile(
        mapResource(await http.post("/llm-model-profiles", toProfileBody(input), resourceDtoSchema))
      );
    },
    async updateProfile(input) {
      return mapLlmModelProfile(
        mapResource(
          await http.patch(
            `/llm-model-profiles/${input.profileId}`,
            toProfileBody(input),
            resourceDtoSchema
          )
        )
      );
    },
    async disableProfile(input) {
      return mapLlmModelProfile(
        mapResource(
          await http.post(
            `/llm-model-profiles/${input.resourceId}/disable`,
            { tenant_id: input.tenantId },
            resourceDtoSchema
          )
        )
      );
    },
    async testProfile(input) {
      const result = await http.post(
        `/llm-model-profiles/${input.profileId}/test`,
        { tenant_id: input.tenantId },
        llmProfileTestResponseSchema
      );
      return {
        success: result.success,
        providerKey: result.provider_key,
        modelName: result.model_name,
        httpStatus: result.http_status ?? undefined,
        latencyMs: result.latency_ms,
        message: result.message
      };
    }
  };
}

function toListQuery(input: LlmListInput) {
  return {
    tenant_id: input.tenantId,
    status: input.status,
    limit: input.limit ?? 100
  };
}

function toProviderBody(input: CreateLlmProviderInput | UpdateLlmProviderInput) {
  return {
    tenant_id: input.tenantId,
    provider_key: "providerKey" in input ? input.providerKey : undefined,
    display_name: input.displayName,
    base_url: input.baseUrl,
    auth_scheme: input.authScheme,
    default_headers_template: input.defaultHeadersTemplate
  };
}

function toProfileBody(input: CreateLlmModelProfileInput | UpdateLlmModelProfileInput) {
  return {
    tenant_id: input.tenantId,
    provider_id: "providerId" in input ? input.providerId : undefined,
    credential_id: input.credentialId,
    profile_name: input.profileName,
    model_name: input.modelName,
    context_window: input.contextWindow,
    max_input_tokens: input.maxInputTokens,
    max_output_tokens: input.maxOutputTokens,
    temperature: input.temperature,
    top_p: input.topP,
    reasoning_effort: input.reasoningEffort,
    response_format: input.responseFormat,
    tool_choice_policy: input.toolChoicePolicy,
    rate_limit_policy: input.rateLimitPolicy,
    cost_policy: input.costPolicy
  };
}

function mapLlmProvider(resource: Resource): LlmProvider {
  const metadata = asRecord(resource.metadata);
  return {
    id: resource.id,
    tenantId: resource.tenantId,
    providerKey: stringFromJson(metadata.provider_key, resource.description ?? ""),
    displayName: resource.name,
    baseUrl: optionalString(metadata.base_url),
    authScheme: stringFromJson(metadata.auth_scheme, "bearer"),
    defaultHeadersTemplate: metadata.default_headers_template ?? {},
    status: resource.status,
    createdAt: resource.createdAt,
    updatedAt: resource.updatedAt,
    raw: resource
  };
}

function mapLlmCredential(resource: Resource): LlmCredential {
  const metadata = asRecord(resource.metadata);
  return {
    id: resource.id,
    tenantId: resource.tenantId,
    label: resource.name,
    providerId: stringFromJson(metadata.provider_id),
    providerKey: optionalString(metadata.provider_key),
    providerName: optionalString(metadata.provider_name) ?? resource.description,
    ownerScope: stringFromJson(metadata.owner_scope, "tenant"),
    ownerResourceId: optionalString(metadata.owner_resource_id),
    hasSecretRef: booleanFromJson(metadata.has_secret_ref),
    hasSecretHash: booleanFromJson(metadata.has_secret_hash),
    expiresAt: optionalString(metadata.expires_at),
    revokedAt: optionalString(metadata.revoked_at),
    status: resource.status,
    createdAt: resource.createdAt,
    updatedAt: resource.updatedAt,
    raw: resource
  };
}

function mapLlmModelProfile(resource: Resource): LlmModelProfile {
  const metadata = asRecord(resource.metadata);
  return {
    id: resource.id,
    tenantId: resource.tenantId,
    profileName: resource.name,
    modelName: resource.description ?? "",
    providerId: stringFromJson(metadata.provider_id),
    credentialId: optionalString(metadata.credential_id),
    contextWindow: optionalNumber(metadata.context_window),
    maxInputTokens: optionalNumber(metadata.max_input_tokens),
    maxOutputTokens: optionalNumber(metadata.max_output_tokens),
    temperature: optionalNumber(metadata.temperature),
    topP: optionalNumber(metadata.top_p),
    reasoningEffort: optionalString(metadata.reasoning_effort),
    responseFormat: metadata.response_format ?? {},
    toolChoicePolicy: metadata.tool_choice_policy ?? {},
    rateLimitPolicy: metadata.rate_limit_policy ?? {},
    costPolicy: metadata.cost_policy ?? {},
    status: resource.status,
    createdAt: resource.createdAt,
    updatedAt: resource.updatedAt,
    raw: resource
  };
}

function optionalString(value: JsonValue | undefined): string | undefined {
  return typeof value === "string" && value ? value : undefined;
}

function optionalNumber(value: JsonValue | undefined): number | undefined {
  return typeof value === "number" ? value : undefined;
}

function booleanFromJson(value: JsonValue | undefined): boolean {
  return typeof value === "boolean" ? value : false;
}
