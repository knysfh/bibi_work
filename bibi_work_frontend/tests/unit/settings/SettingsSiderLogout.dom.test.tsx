/**
 * @vitest-environment jsdom
 */

import React from 'react';
import { fireEvent, render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import SettingsSider from '@/renderer/pages/settings/components/SettingsSider';

const mocks = vi.hoisted(() => ({
  logout: vi.fn(),
  navigate: vi.fn(),
}));

vi.mock('react-i18next', () => ({
  useTranslation: () => ({
    t: (key: string, options?: { defaultValue?: string }) => options?.defaultValue ?? key,
  }),
}));

vi.mock('react-router-dom', () => ({
  useLocation: () => ({ pathname: '/settings/system' }),
  useNavigate: () => mocks.navigate,
}));

vi.mock('@/renderer/hooks/context/AuthContext', () => ({
  useAuth: () => ({ logout: mocks.logout }),
}));

vi.mock('@/renderer/hooks/system/useExtensionSettingsTabs', () => ({
  useExtensionSettingsTabs: () => [],
}));

vi.mock('@/renderer/hooks/system/useExtI18n', () => ({
  useExtI18n: () => ({ resolveExtTabName: () => '' }),
}));

vi.mock('@/renderer/utils/platform', () => ({
  isElectronDesktop: () => true,
  resolveExtensionAssetUrl: () => '',
}));

vi.mock('@/renderer/utils/ui/focus', () => ({
  blurActiveElement: vi.fn(),
}));

vi.mock('@/renderer/utils/ui/siderTooltip', () => ({
  cleanupSiderTooltips: vi.fn(),
  getSiderTooltipProps: () => ({ disabled: true }),
}));

describe('SettingsSider logout placement', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.logout.mockResolvedValue(undefined);
  });

  it('renders Log out in Other immediately before About and invokes logout', () => {
    const { container } = render(<SettingsSider />);
    const logout = screen.getByTestId('desktop-logout');
    const about = container.querySelector<HTMLElement>('[data-settings-id="about"]');

    expect(about).not.toBeNull();
    expect(logout.compareDocumentPosition(about!)).toBe(Node.DOCUMENT_POSITION_FOLLOWING);
    expect(logout).toHaveAccessibleName('common.logout');

    fireEvent.click(logout);

    expect(mocks.logout).toHaveBeenCalledTimes(1);
  });
});
