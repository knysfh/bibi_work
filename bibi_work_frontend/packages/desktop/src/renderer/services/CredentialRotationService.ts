/**
 * Enterprise LLM credential rotation API. Secret references and attempt hashes
 * deliberately do not enter the renderer contract.
 */
import { httpRequest } from '@/common/adapter/httpBridge';

export type CredentialRotationHealth = {
  tenant_id: string;
  worker_enabled: boolean;
  gateway_configured: boolean;
  enabled_credentials: number;
  due_credentials: number;
  running_rotations: number;
  credentials_with_errors: number;
  failed_attempts_24h: number;
  healthy: boolean;
};

export type ManagedLlmCredential = {
  id: string;
  tenant_id: string;
  name: string;
  description?: string | null;
  status: string;
  metadata: {
    provider_id?: string;
    provider_name?: string;
    resolver_scheme?: 'local' | 'env' | 'vault' | 'kms';
    auto_rotation_enabled?: boolean;
    rotation_interval_seconds?: number | null;
    rotate_before_seconds?: number | null;
    next_rotation_at?: string | null;
    rotation_attempts?: number;
    rotation_error?: string | null;
  };
};

export async function loadCredentialRotationOverview(tenantId: string): Promise<{
  health: CredentialRotationHealth;
  credentials: ManagedLlmCredential[];
}> {
  const query = encodeURIComponent(tenantId);
  const [health, credentials] = await Promise.all([
    httpRequest<CredentialRotationHealth>('GET', `/api/v1/llm-credential-rotation/health?tenant_id=${query}`),
    httpRequest<ManagedLlmCredential[]>('GET', `/api/v1/llm-credentials?tenant_id=${query}&limit=500`),
  ]);
  return { health, credentials };
}

export async function updateCredentialRotationPolicy(
  tenantId: string,
  credential: ManagedLlmCredential,
  enabled: boolean
): Promise<ManagedLlmCredential> {
  return httpRequest<ManagedLlmCredential>(
    'POST',
    `/api/v1/llm-credentials/${encodeURIComponent(credential.id)}/rotation-policy`,
    {
      tenant_id: tenantId,
      enabled,
      interval_seconds: enabled ? credential.metadata.rotation_interval_seconds || 30 * 24 * 60 * 60 : null,
      rotate_before_seconds: credential.metadata.rotate_before_seconds ?? 24 * 60 * 60,
    }
  );
}
