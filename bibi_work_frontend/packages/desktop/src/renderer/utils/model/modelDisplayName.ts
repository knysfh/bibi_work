import type { IProvider } from '@/common/config/storage';

export function findConfiguredModelDisplayName(
  providers: IProvider[] | undefined,
  modelReference: string | null | undefined
): string | undefined {
  if (!modelReference) return undefined;

  for (const provider of providers ?? []) {
    for (const modelName of provider.models ?? []) {
      const matchesReference =
        modelReference === modelName ||
        modelReference === `${provider.id}:${modelName}` ||
        modelReference === provider.model_profile_ids?.[modelName];
      if (!matchesReference) continue;

      return provider.model_labels?.[modelName]?.trim() || modelName;
    }
  }

  return undefined;
}

export function getConfiguredModelDisplayName(
  providers: IProvider[] | undefined,
  modelReference: string | null | undefined
): string {
  return findConfiguredModelDisplayName(providers, modelReference) || modelReference || '';
}
