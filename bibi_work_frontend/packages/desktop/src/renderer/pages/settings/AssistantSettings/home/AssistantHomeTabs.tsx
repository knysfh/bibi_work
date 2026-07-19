/**
 * @license
 * Copyright 2025 AionUi (aionui.com)
 * SPDX-License-Identifier: Apache-2.0
 */

import type { AssistantListItem } from '../types';
import MyAssistantsList from './MyAssistantsList';
import OfficialAssistantsGrid from './OfficialAssistantsGrid';
import { useLayoutContext } from '@/renderer/hooks/context/LayoutContext';
import TalkToButlerButton from '@/renderer/components/base/TalkToButlerButton';
import classNames from 'classnames';
import React, { useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';

type AssistantHomeTabsProps = {
  assistants: AssistantListItem[];
  localeKey: string;
  onOpenDetail: (assistant: AssistantListItem) => void;
  onOpenSettings: (assistant: AssistantListItem) => void;
  onDuplicate: (assistant: AssistantListItem) => void;
  onDelete: (assistant: AssistantListItem) => void;
  onCreate: () => void;
  onToggleEnabled: (assistant: AssistantListItem, checked: boolean) => void;
  onReorder: (activeId: string, overId: string) => void | Promise<void>;
  onStartChat: (assistant: AssistantListItem) => void;
  /** Tab to show on mount (e.g. return to Official after editing a builtin). */
  initialTab?: 'mine' | 'official';
  /** Notified whenever the active tab changes, so the parent can remember it. */
  onTabChange?: (tab: 'mine' | 'official') => void;
};

type HomeTab = 'mine' | 'official';

const AssistantHomeTabs: React.FC<AssistantHomeTabsProps> = ({
  assistants,
  localeKey,
  onOpenDetail,
  onOpenSettings,
  onDuplicate,
  onDelete,
  onCreate,
  onToggleEnabled,
  onReorder,
  onStartChat,
  initialTab = 'mine',
  onTabChange,
}) => {
  const { t } = useTranslation();
  const layout = useLayoutContext();
  const isMobile = layout?.isMobile ?? false;
  const [tab, setTab] = useState<HomeTab>(initialTab);

  const selectTab = (next: HomeTab) => {
    setTab(next);
    onTabChange?.(next);
  };

  const counts = useMemo(() => {
    let mine = 0;
    let official = 0;
    for (const assistant of assistants) {
      if (assistant.source === 'builtin') official += 1;
      else mine += 1;
    }
    return { mine, official };
  }, [assistants]);

  const tabButton = (key: HomeTab, label: string, count: number) => (
    <button
      type='button'
      data-testid={`assistant-tab-${key}`}
      onClick={() => selectTab(key)}
      className={`relative inline-flex cursor-pointer items-center border-none bg-transparent px-2px pb-12px text-14px leading-none transition-colors ${
        tab === key ? 'font-600 text-t-primary' : 'font-500 text-t-tertiary hover:text-t-secondary'
      }`}
    >
      <span>{label}</span>
      <span
        className={`ml-6px inline-flex h-16px min-w-16px items-center justify-center rounded-999px px-5px text-10px font-500 leading-none ${
          tab === key ? 'bg-primary-1 text-primary-6' : 'bg-fill-2 text-t-quaternary'
        }`}
      >
        {count}
      </span>
      {tab === key ? <span className='absolute inset-x-0 -bottom-1px h-2px rounded-2px bg-primary-6' /> : null}
    </button>
  );

  return (
    <div data-testid='assistant-home-shell' className='flex h-full min-h-0 flex-col overflow-hidden bg-transparent'>
      <div
        className={`border-b border-border-2 bg-bg-0 ${isMobile ? 'px-16px pt-14px' : 'px-12px pt-24px md:px-40px md:pt-32px'}`}
      >
        <div className='mx-auto w-full max-w-800px'>
          <div className='flex w-full items-center justify-between gap-12px sm:gap-16px'>
            <h1
              className={classNames(
                'm-0 min-w-0 flex-1 font-bold text-t-primary',
                isMobile ? 'text-22px leading-[1.2]' : 'text-28px leading-[1.15]'
              )}
            >
              {t('settings.assistants', { defaultValue: 'Assistants' })}
            </h1>
            <TalkToButlerButton
              className='shrink-0'
              label={t('settings.createAssistant', { defaultValue: 'Create Assistant' })}
              chatLabel={t('settings.talkToButler.createViaChat', { defaultValue: 'Create via chat' })}
              onManual={onCreate}
              manualLabel={t('settings.talkToButler.createManually', { defaultValue: 'Create manually' })}
              prompt={t('settings.talkToButler.prompt.createAssistant', {
                defaultValue: 'Help me create a new assistant and walk me through setting it up.',
              })}
              data-testid='btn-create-assistant'
            />
          </div>
          <p
            className={classNames(
              'm-0 mt-8px w-full text-t-secondary',
              isMobile ? 'text-13px leading-20px' : 'text-14px leading-22px'
            )}
          >
            {t('settings.assistantHomeLeadShort', {
              defaultValue:
                'Ready-to-work AI experts, preloaded with skills. Enable one and it shows up wherever you pick an assistant.',
            })}
          </p>
          <div className='mt-18px flex gap-26px'>
            {tabButton('mine', t('settings.assistantTabMine', { defaultValue: 'My Assistants' }), counts.mine)}
            {tabButton('official', t('settings.assistantTabOfficial', { defaultValue: 'Official' }), counts.official)}
          </div>
        </div>
      </div>

      <div
        data-testid='assistant-home-body'
        className={`min-h-0 flex-1 overflow-auto ${isMobile ? 'px-16px pb-14px pt-14px' : 'px-12px pb-24px pt-18px md:px-40px'}`}
      >
        <div className='mx-auto w-full max-w-800px'>
          {tab === 'mine' ? (
            <MyAssistantsList
              assistants={assistants}
              localeKey={localeKey}
              onOpenDetail={onOpenDetail}
              onDelete={onDelete}
              onToggleEnabled={onToggleEnabled}
              onReorder={onReorder}
              onStartChat={onStartChat}
              onGoOfficial={() => selectTab('official')}
            />
          ) : (
            <OfficialAssistantsGrid
              assistants={assistants}
              localeKey={localeKey}
              onOpenSettings={onOpenSettings}
              onDuplicate={onDuplicate}
              onToggleEnabled={onToggleEnabled}
              onStartChat={onStartChat}
            />
          )}
        </div>
      </div>
    </div>
  );
};

export default AssistantHomeTabs;
