/**
 * @license
 * Copyright 2025 AionUi (aionui.com)
 * SPDX-License-Identifier: Apache-2.0
 */

import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Alert, Button, Checkbox, Empty, Message, Modal, Spin, Tag, Typography } from '@arco-design/web-react';
import { useTranslation } from 'react-i18next';
import { acpConversation, type AgentMcpCapabilities, type AgentMcpCapabilityTool } from '@/common/adapter/ipcBridge';

type AgentMcpCapabilitiesPanelProps = {
  agentId: string;
  onPublished?: () => void;
};

const riskColor = (risk: AgentMcpCapabilityTool['risk_level']) => {
  if (risk === 'low') return 'green';
  if (risk === 'medium') return 'orange';
  return 'red';
};

const sameSelection = (left: Iterable<string>, right: Iterable<string>) => {
  const leftIds = Array.from(left).toSorted();
  const rightIds = Array.from(right).toSorted();
  return leftIds.length === rightIds.length && leftIds.every((id, index) => id === rightIds[index]);
};

const AgentMcpCapabilitiesPanel: React.FC<AgentMcpCapabilitiesPanelProps> = ({ agentId, onPublished }) => {
  const { t } = useTranslation();
  const [capabilities, setCapabilities] = useState<AgentMcpCapabilities | null>(null);
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());
  const [browserEnabled, setBrowserEnabled] = useState(false);
  const [isLoading, setIsLoading] = useState(true);
  const [isSaving, setIsSaving] = useState(false);
  const [error, setError] = useState('');
  const initialSelectedRef = useRef<Set<string>>(new Set());
  const initialBrowserEnabledRef = useRef(false);

  const loadCapabilities = useCallback(async () => {
    setIsLoading(true);
    setError('');
    try {
      const result = await acpConversation.getAgentMcpCapabilities.invoke({ id: agentId });
      const selected = new Set(result.selected_mcp_tool_ids);
      setCapabilities(result);
      setSelectedIds(selected);
      setBrowserEnabled(result.browser_enabled);
      initialSelectedRef.current = selected;
      initialBrowserEnabledRef.current = result.browser_enabled;
    } catch (loadError) {
      console.error('Failed to load Agent MCP capabilities:', loadError);
      setError(loadError instanceof Error ? loadError.message : String(loadError));
    } finally {
      setIsLoading(false);
    }
  }, [agentId]);

  useEffect(() => {
    void loadCapabilities();
  }, [loadCapabilities]);

  const allTools = useMemo(() => capabilities?.servers.flatMap((server) => server.tools) ?? [], [capabilities]);
  const hasChanges =
    browserEnabled !== initialBrowserEnabledRef.current || !sameSelection(selectedIds, initialSelectedRef.current);

  const toggleTool = useCallback((toolId: string, checked: boolean) => {
    setSelectedIds((current) => {
      const next = new Set(current);
      if (checked) next.add(toolId);
      else next.delete(toolId);
      return next;
    });
  }, []);

  const publish = useCallback(async () => {
    if (isSaving) return;
    setIsSaving(true);
    setError('');
    try {
      const result = await acpConversation.publishAgentMcpCapabilities.invoke({
        id: agentId,
        mcp_tool_ids: Array.from(selectedIds),
        browser_enabled: browserEnabled,
      });
      await loadCapabilities();
      onPublished?.();
      Message.success(
        result.previous_version_revoked ? t('settings.agentMcp.publishedAndRevoked') : t('settings.agentMcp.published')
      );
    } catch (saveError) {
      console.error('Failed to publish Agent MCP capabilities:', saveError);
      setError(saveError instanceof Error ? saveError.message : String(saveError));
      throw saveError;
    } finally {
      setIsSaving(false);
    }
  }, [agentId, browserEnabled, isSaving, loadCapabilities, onPublished, selectedIds, t]);

  const handlePublish = useCallback(() => {
    const newlySelectedHighRiskTools = allTools.filter(
      (tool) => tool.risk_level === 'high' && selectedIds.has(tool.id) && !initialSelectedRef.current.has(tool.id)
    );
    const browserNewlyEnabled = browserEnabled && !initialBrowserEnabledRef.current;
    if (newlySelectedHighRiskTools.length === 0 && !browserNewlyEnabled) {
      void publish();
      return;
    }
    Modal.confirm({
      title: t('settings.agentMcp.capabilityConfirmTitle'),
      content: (
        <div className='space-y-8px'>
          {browserNewlyEnabled ? <div>{t('settings.agentMcp.browserConfirmContent')}</div> : null}
          {newlySelectedHighRiskTools.length ? (
            <div>
              {t('settings.agentMcp.highRiskConfirmContent', {
                names: newlySelectedHighRiskTools.map((tool) => tool.name).join(', '),
              })}
            </div>
          ) : null}
        </div>
      ),
      okButtonProps: { status: 'danger' },
      okText: t('settings.agentMcp.publish'),
      onOk: publish,
      wrapClassName: 'modal-publish-agent-mcp-capabilities',
    });
  }, [allTools, browserEnabled, publish, selectedIds, t]);

  return (
    <section
      data-testid='agent-mcp-capabilities-panel'
      className='mt-18px min-w-0 rounded-10px bg-aou-1 px-12px py-12px'
    >
      <div className='flex min-w-0 flex-wrap items-start justify-between gap-8px'>
        <div className='min-w-0 flex-1'>
          <Typography.Title heading={6} className='!mb-2px !mt-0 !text-14px !font-600 !text-t-primary'>
            {t('settings.agentMcp.title')}
          </Typography.Title>
          <Typography.Text className='block text-11px leading-17px text-t-tertiary'>
            {t('settings.agentMcp.description')}
          </Typography.Text>
        </div>
        <Tag size='small' color='arcoblue' className='!m-0 !flex-shrink-0'>
          {t('settings.agentMcp.selectedCount', { selected: selectedIds.size, total: allTools.length })}
        </Tag>
      </div>

      {capabilities?.stale_mcp_tool_ids.length ? (
        <Alert type='warning' className='!mt-10px !rounded-8px' content={t('settings.agentMcp.staleWarning')} />
      ) : null}
      {error ? (
        <Alert type='error' className='!mt-10px !rounded-8px' content={error} closable onClose={() => setError('')} />
      ) : null}

      <label
        data-testid='agent-browser-capability'
        className='mt-10px flex min-w-0 cursor-pointer items-start gap-9px rounded-8px border border-border-2 bg-base px-10px py-10px hover:bg-fill-1'
      >
        <Checkbox
          data-testid='agent-browser-capability-checkbox'
          className='mt-1px flex-shrink-0'
          checked={browserEnabled}
          disabled={isLoading}
          onChange={setBrowserEnabled}
        />
        <div className='min-w-0 flex-1'>
          <div className='flex min-w-0 flex-wrap items-center gap-5px'>
            <span className='text-12px font-600 text-t-primary'>{t('settings.agentMcp.browserTitle')}</span>
            <Tag size='small' color='orange' className='!m-0 !flex-shrink-0'>
              {t('settings.agentMcp.risk.medium')}
            </Tag>
          </div>
          <div className='mt-2px break-words text-10px leading-15px text-t-tertiary'>
            {t('settings.agentMcp.browserDescription')}
          </div>
        </div>
      </label>

      {isLoading ? (
        <div className='flex min-h-120px items-center justify-center'>
          <Spin size={20} />
        </div>
      ) : allTools.length === 0 ? (
        <div className='mt-10px rounded-8px bg-aou-2 py-18px'>
          <Empty description={t('settings.agentMcp.empty')} />
        </div>
      ) : (
        <div className='mt-10px max-h-420px min-w-0 overflow-y-auto overflow-x-hidden rounded-8px border border-border-2 bg-base'>
          {capabilities?.servers.map((server) => (
            <div key={server.id} data-testid={`agent-mcp-server-${server.id}`} className='min-w-0'>
              <div className='border-b border-border-2 bg-aou-2 px-10px py-8px'>
                <div className='truncate text-12px font-600 text-t-primary' title={server.name}>
                  {server.name}
                </div>
                {server.description ? (
                  <div className='mt-2px line-clamp-2 break-words text-10px leading-15px text-t-tertiary'>
                    {server.description}
                  </div>
                ) : null}
              </div>
              {server.tools.map((tool) => (
                <label
                  key={tool.id}
                  data-testid={`agent-mcp-tool-${tool.id}`}
                  className='flex min-w-0 cursor-pointer items-start gap-9px border-b border-border-2 px-10px py-9px last:border-b-0 hover:bg-fill-1'
                >
                  <Checkbox
                    data-testid={`agent-mcp-tool-checkbox-${tool.id}`}
                    className='mt-1px flex-shrink-0'
                    checked={selectedIds.has(tool.id)}
                    onChange={(checked) => toggleTool(tool.id, checked)}
                  />
                  <div className='min-w-0 flex-1'>
                    <div className='flex min-w-0 flex-wrap items-center gap-5px'>
                      <span className='min-w-0 break-all text-12px font-500 text-t-primary'>{tool.name}</span>
                      <Tag size='small' color={riskColor(tool.risk_level)} className='!m-0 !flex-shrink-0'>
                        {t(`settings.agentMcp.risk.${tool.risk_level}`)}
                      </Tag>
                      {tool.stale ? (
                        <Tag size='small' color='orange' className='!m-0 !flex-shrink-0'>
                          {t('settings.agentMcp.schemaChanged')}
                        </Tag>
                      ) : null}
                    </div>
                    <div className='mt-2px line-clamp-2 break-words text-10px leading-15px text-t-tertiary'>
                      {tool.description || t('settings.agentMcp.noDescription')}
                    </div>
                  </div>
                </label>
              ))}
            </div>
          ))}
        </div>
      )}

      <div className='mt-10px flex flex-wrap items-center justify-between gap-8px'>
        <div className='flex flex-wrap gap-6px'>
          <Button
            size='small'
            disabled={isLoading || allTools.length === 0}
            onClick={() => setSelectedIds(new Set(allTools.filter((tool) => tool.read_only).map((tool) => tool.id)))}
            className='!rounded-8px'
          >
            {t('settings.agentMcp.selectReadOnly')}
          </Button>
          <Button
            size='small'
            disabled={isLoading || selectedIds.size === 0}
            onClick={() => setSelectedIds(new Set())}
            className='!rounded-8px'
          >
            {t('settings.agentMcp.clear')}
          </Button>
        </div>
        <Button
          data-testid='btn-publish-agent-mcp-capabilities'
          type='primary'
          size='small'
          disabled={isLoading || isSaving || !hasChanges}
          loading={isSaving}
          onClick={handlePublish}
          className='!rounded-8px'
        >
          {t('settings.agentMcp.publish')}
        </Button>
      </div>
    </section>
  );
};

export default AgentMcpCapabilitiesPanel;
