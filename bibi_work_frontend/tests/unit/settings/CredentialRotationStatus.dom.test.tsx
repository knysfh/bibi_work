/** @vitest-environment jsdom */
import React from 'react';
import { ConfigProvider } from '@arco-design/web-react';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const mocks = vi.hoisted(() => ({
  load: vi.fn(),
  update: vi.fn(),
}));

vi.mock('react-i18next', () => ({
  useTranslation: () => ({
    t: (_key: string, options?: Record<string, unknown>) =>
      String(options?.defaultValue || _key).replace('{{provider}}', String(options?.provider || '')),
  }),
}));

vi.mock('@/renderer/hooks/workbench/useWorkbenchFeatureFlags', () => ({
  useWorkbenchBootstrap: () => ({ data: { auth: { tenant_id: 'tenant-1' } } }),
}));

vi.mock('@/renderer/services/CredentialRotationService', () => ({
  loadCredentialRotationOverview: mocks.load,
  updateCredentialRotationPolicy: mocks.update,
}));

import CredentialRotationStatus from '@/renderer/components/settings/SettingsModal/contents/CredentialRotationStatus';

const overview = (configured: boolean) => ({
  health: {
    tenant_id: 'tenant-1',
    worker_enabled: configured,
    gateway_configured: configured,
    enabled_credentials: 0,
    due_credentials: 0,
    running_rotations: 0,
    credentials_with_errors: 0,
    failed_attempts_24h: 0,
    healthy: true,
  },
  credentials: [
    {
      id: 'credential-1',
      tenant_id: 'tenant-1',
      name: 'credential credential-1',
      description: 'OpenAI',
      status: 'active',
      metadata: { provider_name: 'OpenAI', auto_rotation_enabled: false },
    },
  ],
});

describe('CredentialRotationStatus', () => {
  beforeEach(() => {
    cleanup();
    vi.clearAllMocks();
    mocks.update.mockResolvedValue(undefined);
  });

  it('shows backend readiness and blocks enablement when the gateway is unavailable', async () => {
    mocks.load.mockResolvedValue(overview(false));
    render(
      <ConfigProvider>
        <CredentialRotationStatus />
      </ConfigProvider>
    );

    expect(await screen.findByTestId('credential-rotation-card')).toHaveTextContent('Worker disabled');
    fireEvent.click(screen.getByRole('switch', { name: 'Automatic rotation for OpenAI' }));
    expect(mocks.update).not.toHaveBeenCalled();
  });

  it('enables the default rotation policy and refreshes status', async () => {
    mocks.load.mockResolvedValue(overview(true));
    render(
      <ConfigProvider>
        <CredentialRotationStatus />
      </ConfigProvider>
    );

    fireEvent.click(await screen.findByRole('switch', { name: 'Automatic rotation for OpenAI' }));
    await waitFor(() => {
      expect(mocks.update).toHaveBeenCalledWith('tenant-1', expect.objectContaining({ id: 'credential-1' }), true);
    });
    expect(mocks.load).toHaveBeenCalledTimes(2);
  });
});
