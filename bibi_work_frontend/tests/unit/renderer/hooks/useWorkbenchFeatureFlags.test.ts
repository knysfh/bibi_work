/**
 * @license
 * Copyright 2025 AionUi (aionui.com)
 * SPDX-License-Identifier: Apache-2.0
 */

import { describe, expect, it, vi } from 'vitest';

vi.mock('@/common', () => ({
  ipcBridge: {
    workbench: {
      bootstrap: {
        invoke: vi.fn(),
      },
    },
  },
}));

import { getWorkbenchFeatureFlag } from '@/renderer/hooks/workbench/useWorkbenchFeatureFlags';

describe('getWorkbenchFeatureFlag', () => {
  it('returns a nested boolean flag value', () => {
    expect(getWorkbenchFeatureFlag({ desktop: { cdp_remote_control: true } }, 'desktop.cdp_remote_control')).toBe(true);
    expect(
      getWorkbenchFeatureFlag({ desktop: { cdp_remote_control: false } }, 'desktop.cdp_remote_control', true)
    ).toBe(false);
  });

  it('uses the fallback for missing or non-boolean values', () => {
    expect(getWorkbenchFeatureFlag({ desktop: {} }, 'desktop.cdp_remote_control')).toBe(false);
    expect(getWorkbenchFeatureFlag({ desktop: { cdp_remote_control: 'true' } }, 'desktop.cdp_remote_control')).toBe(
      false
    );
    expect(getWorkbenchFeatureFlag(undefined, 'desktop.cdp_remote_control', true)).toBe(true);
  });
});
