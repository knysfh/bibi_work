/**
 * @license
 * Copyright 2025 AionUi (aionui.com)
 * SPDX-License-Identifier: Apache-2.0
 */

import { describe, expect, it } from 'vitest';
import { getPlatformByValue, getProviderLogo, MODEL_PLATFORMS } from '@/renderer/utils/model/modelPlatforms';

describe('modelPlatforms', () => {
  it('does not emit legacy backend logo API paths without an asset owner', () => {
    const logos = MODEL_PLATFORMS.map((platform) => platform.logo).filter((logo): logo is string => Boolean(logo));

    expect(logos.some((logo) => logo.includes('/api/assets/logos/'))).toBe(false);
  });

  it('uses the built-in UI fallback for preset providers when logo assets are absent', () => {
    expect(getPlatformByValue('gemini')?.logo).toBeNull();
    expect(getProviderLogo({ name: 'Gemini', base_url: 'https://generativelanguage.googleapis.com' })).toBeNull();
  });
});
