/**
 * Compact enterprise credential-rotation control for model settings.
 */
import { isBackendHttpError } from '@/common/adapter/httpBridge';
import { useWorkbenchBootstrap } from '@/renderer/hooks/workbench/useWorkbenchFeatureFlags';
import {
  loadCredentialRotationOverview,
  updateCredentialRotationPolicy,
  type ManagedLlmCredential,
} from '@/renderer/services/CredentialRotationService';
import { Button, Message, Spin, Switch, Tag, Tooltip } from '@arco-design/web-react';
import { Refresh } from '@icon-park/react';
import React, { useCallback, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';

const formatNextRotation = (value: string | null | undefined, unscheduled: string, scheduled: string): string => {
  if (!value) return unscheduled;
  const parsed = new Date(value);
  return Number.isNaN(parsed.getTime()) ? scheduled : parsed.toLocaleString();
};

const CredentialRotationStatus: React.FC = () => {
  const { t } = useTranslation();
  const { data: bootstrap } = useWorkbenchBootstrap();
  const tenantId = bootstrap?.auth?.tenant_id;
  const [overview, setOverview] = useState<Awaited<ReturnType<typeof loadCredentialRotationOverview>> | null>(null);
  const [loading, setLoading] = useState(false);
  const [savingId, setSavingId] = useState<string | null>(null);
  const [message, messageContext] = Message.useMessage();

  const refresh = useCallback(async () => {
    if (!tenantId) return;
    setLoading(true);
    try {
      setOverview(await loadCredentialRotationOverview(tenantId));
    } catch (error) {
      console.warn('[credential-rotation] failed to load health', error);
      setOverview(null);
    } finally {
      setLoading(false);
    }
  }, [tenantId]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  if (!tenantId || (!overview && !loading)) return null;

  const changePolicy = async (credential: ManagedLlmCredential, enabled: boolean) => {
    if (enabled && (!overview?.health.worker_enabled || !overview.health.gateway_configured)) {
      message.warning(
        t('settings.credentialRotation.notConfigured', {
          defaultValue: 'Automatic rotation is not configured on this server.',
        })
      );
      return;
    }
    setSavingId(credential.id);
    try {
      await updateCredentialRotationPolicy(tenantId, credential, enabled);
      message.success(
        enabled
          ? t('settings.credentialRotation.enabled', { defaultValue: 'Automatic credential rotation enabled.' })
          : t('settings.credentialRotation.disabled', { defaultValue: 'Automatic credential rotation disabled.' })
      );
      await refresh();
    } catch (error) {
      const detail =
        isBackendHttpError(error) && error.backendMessage
          ? error.backendMessage
          : t('settings.credentialRotation.updateFailed', { defaultValue: 'Policy update failed.' });
      message.error(detail);
    } finally {
      setSavingId(null);
    }
  };

  const health = overview?.health;
  const activeCredentials = (overview?.credentials || []).filter((credential) => credential.status !== 'revoked');

  return (
    <div
      data-testid='credential-rotation-card'
      className='rd-12px border border-solid border-[var(--color-border-2)] bg-[var(--color-bg-2)] px-14px py-12px'
    >
      {messageContext}
      <div className='flex items-center justify-between gap-8px'>
        <div>
          <div className='flex items-center gap-8px'>
            <span className='text-14px font-500 text-t-primary'>
              {t('settings.credentialRotation.title', { defaultValue: 'Automatic credential rotation' })}
            </span>
            {health && (
              <Tag color={health.healthy ? 'green' : 'red'}>
                {health.healthy
                  ? t('settings.credentialRotation.healthy', { defaultValue: 'Healthy' })
                  : t('settings.credentialRotation.actionRequired', { defaultValue: 'Action required' })}
              </Tag>
            )}
          </div>
          <div className='text-12px text-t-secondary mt-4px'>
            {t('settings.credentialRotation.worker', { defaultValue: 'Worker' })}{' '}
            {health?.worker_enabled
              ? t('settings.credentialRotation.workerEnabled', { defaultValue: 'enabled' })
              : t('settings.credentialRotation.workerDisabled', { defaultValue: 'disabled' })}{' '}
            · {t('settings.credentialRotation.gateway', { defaultValue: 'Gateway' })}{' '}
            {health?.gateway_configured
              ? t('settings.credentialRotation.gatewayConfigured', { defaultValue: 'configured' })
              : t('settings.credentialRotation.gatewayNotConfigured', { defaultValue: 'not configured' })}
            {health && health.failed_attempts_24h > 0
              ? ` · ${t('settings.credentialRotation.failedIn24h', {
                  count: health.failed_attempts_24h,
                  defaultValue: '{{count}} failed in 24h',
                })}`
              : ''}
          </div>
        </div>
        <Button
          size='mini'
          type='text'
          aria-label={t('settings.credentialRotation.refresh', { defaultValue: 'Refresh credential rotation status' })}
          icon={<Refresh size='15' />}
          loading={loading}
          onClick={() => void refresh()}
        />
      </div>

      {loading && !overview ? (
        <div className='flex justify-center py-10px'>
          <Spin size={18} />
        </div>
      ) : activeCredentials.length === 0 ? (
        <div className='text-12px text-t-secondary mt-10px'>
          {t('settings.credentialRotation.noCredentials', { defaultValue: 'No managed LLM credentials.' })}
        </div>
      ) : (
        <div className='mt-10px flex flex-col gap-8px'>
          {activeCredentials.map((credential) => {
            const enabled = credential.metadata.auto_rotation_enabled === true;
            const hasError = Boolean(credential.metadata.rotation_error);
            return (
              <div
                key={credential.id}
                className='flex items-center justify-between gap-12px border-t border-[var(--color-border-2)] pt-8px'
              >
                <div className='min-w-0'>
                  <div className='text-13px text-t-primary truncate'>
                    {credential.metadata.provider_name || credential.description || credential.name}
                    {hasError && (
                      <Tag className='ml-6px' color='red'>
                        {t('settings.credentialRotation.retryPending', { defaultValue: 'Retry pending' })}
                      </Tag>
                    )}
                  </div>
                  <div className='text-11px text-t-secondary mt-2px'>
                    {enabled
                      ? `${t('settings.credentialRotation.next', { defaultValue: 'Next' })}: ${formatNextRotation(
                          credential.metadata.next_rotation_at,
                          t('settings.credentialRotation.notScheduled', { defaultValue: 'Not scheduled' }),
                          t('settings.credentialRotation.scheduled', { defaultValue: 'Scheduled' })
                        )}`
                      : t('settings.credentialRotation.policyAvailable', { defaultValue: '30-day policy available' })}
                  </div>
                </div>
                <Tooltip
                  content={
                    enabled
                      ? t('settings.credentialRotation.disableAction', { defaultValue: 'Disable automatic rotation' })
                      : t('settings.credentialRotation.enableAction', {
                          defaultValue: 'Enable a 30-day rotation policy',
                        })
                  }
                >
                  <Switch
                    size='small'
                    aria-label={t('settings.credentialRotation.credentialSwitch', {
                      provider: credential.metadata.provider_name || credential.name,
                      defaultValue: 'Automatic rotation for {{provider}}',
                    })}
                    checked={enabled}
                    loading={savingId === credential.id}
                    onChange={(checked) => void changePolicy(credential, checked)}
                  />
                </Tooltip>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
};

export default CredentialRotationStatus;
