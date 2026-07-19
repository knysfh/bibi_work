/**
 * @license
 * Copyright 2025 AionUi (aionui.com)
 * SPDX-License-Identifier: Apache-2.0
 */

import type { Assistant } from '@/common/types/agent/assistantTypes';

/**
 * Single source of truth for which assistants appear in a *selection* list
 * (home pill bar, team creation, scheduled-task dropdown, …) and in what order.
 *
 * Rules (see PRD F-AHM-06 / F-AHM-07):
 *  - Only enabled assistants are selectable.
 *  - Groups are ordered: bare CLI first, then user-created, then official
 *    (builtin). This keeps the user's own CLI agents from being buried under
 *    official templates. Cross-group order is fixed and not user-adjustable.
 *  - Within a group, order follows `sort_order` (which the user controls for
 *    CLI/user via drag; official order is manifest-owned by the backend).
 *
 * Note: a bare CLI assistant surfaces with `source === 'generated'`.
 */

/** Group weight — lower comes first. Bare CLI < user-created < official. */
const sourceGroupWeight = (source: string): number => {
  switch (source) {
    case 'generated':
      return 0;
    case 'user':
      return 1;
    case 'builtin':
      return 2;
    default:
      return 1;
  }
};

/**
 * Return the enabled assistants ordered for a selection list:
 * bare → user → builtin, each group sorted by `sort_order`.
 */
export const selectableAssistants = (assistants: Assistant[]): Assistant[] =>
  [...assistants]
    .filter((assistant) => assistant.enabled !== false)
    .sort((left, right) => {
      const groupDelta = sourceGroupWeight(left.source) - sourceGroupWeight(right.source);
      if (groupDelta !== 0) return groupDelta;
      return left.sort_order - right.sort_order;
    });
