/**
 * @license
 * Copyright 2025 AionUi (aionui.com)
 * SPDX-License-Identifier: Apache-2.0
 */

import type { ICronAgentConfigWrite } from '@/common/adapter/ipcBridge';
import type { Assistant } from '@/common/types/agent/assistantTypes';
import { resolveAssistantName } from '@renderer/utils/model/assistantDisplay';

type ResolveCronAgentConfigInput = {
  agentValue: string;
  presetAssistants: Assistant[];
  model_id?: string;
  config_options?: Record<string, string>;
  workspace?: string;
  localeKey?: string;
  getMode: (assistant: Assistant) => string | undefined;
};

type ResolveCronAgentConfigResult = {
  agent_config: ICronAgentConfigWrite | undefined;
};

export function resolveCronAgentConfig(input: ResolveCronAgentConfigInput): ResolveCronAgentConfigResult {
  const { agentValue, presetAssistants, model_id, config_options, workspace, localeKey = 'en-US', getMode } = input;

  const colonIdx = agentValue.indexOf(':');
  const prefixedId = colonIdx >= 0 ? agentValue.substring(colonIdx + 1) : agentValue;
  const assistantSelection = presetAssistants.find((item) => item.id === prefixedId || item.id === agentValue);
  if (!assistantSelection) {
    throw new Error('assistant_id is required');
  }

  const assistant = assistantSelection;
  const assistantName = resolveAssistantName(assistant, localeKey, assistant.name);
  const mode = getMode(assistant);
  const agent_config: ICronAgentConfigWrite = {
    name: assistantName,
    assistant_id: assistant.id,
    mode,
    model_id,
    config_options,
    workspace,
  };

  return { agent_config };
}
