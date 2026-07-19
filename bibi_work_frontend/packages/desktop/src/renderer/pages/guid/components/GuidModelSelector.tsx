/**
 * @license
 * Copyright 2025 AionUi (aionui.com)
 * SPDX-License-Identifier: Apache-2.0
 */

import { iconColors } from '@/renderer/styles/colors';
import { getModelDisplayLabel } from '@/renderer/utils/model/agentLogo';
import type { AgentRuntimeDerivedOption } from '@/renderer/utils/model/agentRuntimeCatalog';
import type { AcpModelInfo } from '../types';
import { Button, Dropdown, Menu, Tooltip } from '@arco-design/web-react';
import { Brain, Down } from '@icon-park/react';
import React from 'react';
import { useTranslation } from 'react-i18next';
import { useProvidersQuery } from '@/renderer/hooks/agent/useModelProviderList';
import {
  composeRuntimeSelectorLabel,
  RuntimeSelectorCheckedItem,
  RuntimeSelectorMenuDivider,
  renderThoughtLevelMenuGroup,
} from '@/renderer/components/agent/runtimeSelectorOptions';

type GuidModelSelectorProps = {
  currentAcpCachedModelInfo: AcpModelInfo | null;
  selectedAcpModel: string | null;
  setSelectedAcpModel: React.Dispatch<React.SetStateAction<string | null>>;
  thoughtLevelOption?: AgentRuntimeDerivedOption | null;
  onThoughtLevelSelect?: (value: string) => void;
};

const GuidModelSelector: React.FC<GuidModelSelectorProps> = ({
  currentAcpCachedModelInfo,
  selectedAcpModel,
  setSelectedAcpModel,
  thoughtLevelOption,
  onThoughtLevelSelect,
}) => {
  const { t } = useTranslation();
  const defaultModelLabel = t('common.defaultModel');

  // 获取模型配置数据（包含健康状态）
  const { data: modelConfig } = useProvidersQuery();

  const acpSelectedLabel = React.useMemo(() => {
    return (
      currentAcpCachedModelInfo?.available_models?.find((m) => m.id === selectedAcpModel)?.label ||
      currentAcpCachedModelInfo?.current_model_label ||
      currentAcpCachedModelInfo?.current_model_id ||
      ''
    );
  }, [
    currentAcpCachedModelInfo?.available_models,
    currentAcpCachedModelInfo?.current_model_id,
    currentAcpCachedModelInfo?.current_model_label,
    selectedAcpModel,
  ]);

  const acpButtonLabel = React.useMemo(() => {
    return getModelDisplayLabel({
      selected_value: selectedAcpModel || currentAcpCachedModelInfo?.current_model_id,
      selectedLabel: acpSelectedLabel,
      defaultModelLabel,
      fallbackLabel: defaultModelLabel,
    });
  }, [acpSelectedLabel, currentAcpCachedModelInfo?.current_model_id, defaultModelLabel, selectedAcpModel]);
  const selectedThoughtLevelValue = thoughtLevelOption?.currentValue || thoughtLevelOption?.options[0]?.value || '';
  const normalizedThoughtLevelOption =
    thoughtLevelOption && thoughtLevelOption.options.length > 0
      ? {
          ...thoughtLevelOption,
          currentValue: selectedThoughtLevelValue || null,
        }
      : null;
  const combinedAcpButtonLabel = composeRuntimeSelectorLabel({
    modelLabel: acpButtonLabel,
    thoughtLevel: normalizedThoughtLevelOption,
  });

  // ACP cached model selector
  if (currentAcpCachedModelInfo && currentAcpCachedModelInfo.available_models?.length > 0) {
    if (currentAcpCachedModelInfo.available_models.length > 0) {
      return (
        <Dropdown
          trigger='click'
          droplist={
            <Menu selectedKeys={selectedAcpModel ? [selectedAcpModel] : []}>
              {renderThoughtLevelMenuGroup({
                thoughtLevel: normalizedThoughtLevelOption,
                title: t('agent.thoughtLevel.label'),
                onSelect: (value) => onThoughtLevelSelect?.(value),
              })}
              {normalizedThoughtLevelOption ? <RuntimeSelectorMenuDivider /> : null}
              <Menu.ItemGroup title={t('common.model', { defaultValue: 'Model' })}>
                {currentAcpCachedModelInfo.available_models.map((model) => {
                  // 获取模型健康状态
                  const providerConfig = modelConfig?.find((p) => p.platform?.includes(''));
                  const healthStatus = providerConfig?.model_health?.[model.id]?.status || 'unknown';
                  const healthColor =
                    healthStatus === 'healthy'
                      ? 'bg-green-500'
                      : healthStatus === 'unhealthy'
                        ? 'bg-red-500'
                        : 'bg-gray-400';

                  return (
                    <Menu.Item
                      key={model.id}
                      className={model.id === selectedAcpModel ? '!bg-2' : ''}
                      onClick={() => setSelectedAcpModel(model.id)}
                    >
                      <div className='flex items-center gap-8px w-full'>
                        {healthStatus !== 'unknown' && (
                          <div className={`w-6px h-6px rounded-full shrink-0 ${healthColor}`} />
                        )}
                        <RuntimeSelectorCheckedItem
                          selected={model.id === selectedAcpModel}
                          description={model.description}
                        >
                          {model.label}
                        </RuntimeSelectorCheckedItem>
                      </div>
                    </Menu.Item>
                  );
                })}
              </Menu.ItemGroup>
            </Menu>
          }
        >
          <Button className={'sendbox-model-btn guid-config-btn'} shape='round' size='small'>
            <span className='flex items-center gap-6px min-w-0'>
              <Brain theme='outline' size='14' fill={iconColors.secondary} className='shrink-0' />
              <span>{combinedAcpButtonLabel}</span>
              <Down theme='outline' size='12' fill={iconColors.secondary} className='shrink-0' />
            </span>
          </Button>
        </Dropdown>
      );
    }

    return (
      <Tooltip content={t('conversation.welcome.modelSwitchNotSupported')} position='top'>
        <Button
          className={'sendbox-model-btn guid-config-btn'}
          shape='round'
          size='small'
          style={{ cursor: 'default' }}
        >
          <span className='flex items-center gap-6px min-w-0'>
            <Brain theme='outline' size='14' fill={iconColors.secondary} className='shrink-0' />
            <span>{acpButtonLabel}</span>
          </span>
        </Button>
      </Tooltip>
    );
  }

  // Fallback: no model switching
  return (
    <Tooltip content={t('conversation.welcome.modelSwitchNotSupported')} position='top'>
      <Button className={'sendbox-model-btn guid-config-btn'} shape='round' size='small' style={{ cursor: 'default' }}>
        <span className='flex items-center gap-6px min-w-0'>
          <Brain theme='outline' size='14' fill={iconColors.secondary} className='shrink-0' />
          <span>{defaultModelLabel}</span>
        </span>
      </Button>
    </Tooltip>
  );
};

export default GuidModelSelector;
