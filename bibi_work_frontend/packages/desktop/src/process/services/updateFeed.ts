/**
 * @license
 * Copyright 2025 AionUi (aionui.com)
 * SPDX-License-Identifier: Apache-2.0
 */

export const UPDATE_REPOSITORY = 'knysfh/bibi_work';

export function buildUpdateFeedOptions() {
  return {
    provider: 'github' as const,
    owner: 'knysfh',
    repo: 'bibi_work',
  };
}
