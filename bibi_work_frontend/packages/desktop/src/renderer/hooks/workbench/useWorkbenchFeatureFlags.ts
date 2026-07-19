/**
 * @license
 * Copyright 2025 AionUi (aionui.com)
 * SPDX-License-Identifier: Apache-2.0
 */

import { ipcBridge } from '@/common';
import type { WorkbenchBootstrap } from '@/common/adapter/ipcBridge';
import useSWR from 'swr';

const WORKBENCH_BOOTSTRAP_SWR_KEY = 'workbench.bootstrap';

export function getWorkbenchFeatureFlag(flags: unknown, path: string, fallback = false): boolean {
  const value = path.split('.').reduce<unknown>((current, part) => {
    if (!current || typeof current !== 'object') return undefined;
    return (current as Record<string, unknown>)[part];
  }, flags);
  return typeof value === 'boolean' ? value : fallback;
}

export function useWorkbenchBootstrap() {
  return useSWR<WorkbenchBootstrap | null>(
    WORKBENCH_BOOTSTRAP_SWR_KEY,
    (): Promise<WorkbenchBootstrap | null> => ipcBridge.workbench.bootstrap.invoke().catch((): null => null),
    {
      revalidateOnFocus: false,
      dedupingInterval: 60_000,
    }
  );
}

export function useWorkbenchFeatureFlag(path: string, fallback = false): { enabled: boolean; isLoading: boolean } {
  const { data, isLoading } = useWorkbenchBootstrap();
  return {
    enabled: getWorkbenchFeatureFlag(data?.feature_flags, path, fallback),
    isLoading,
  };
}
