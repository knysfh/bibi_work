/**
 * @license
 * Copyright 2025 AionUi (aionui.com)
 * SPDX-License-Identifier: Apache-2.0
 */

import { httpGet, httpPost, httpPut, withResponseMap, wsMappedEmitter } from './httpBridge';

import type {
  IChannelAssistantBindingWrite,
  IChannelDefaultModelSetting,
  IChannelIngressMessageRequest,
  IChannelIngressMessageResponse,
  IChannelPairingDecision,
  IChannelPairingRequest,
  IChannelPlatformSettings,
  IChannelPluginStatus,
  IChannelSettingsSyncResponse,
  IChannelSession,
  IChannelUser,
  IChannelUserRevocation,
} from '@/common/types/channel/channel';

type RawPluginStatus = Record<string, unknown>;
type RawPairing = Record<string, unknown>;
type RawPairingDecision = Record<string, unknown> & { user?: RawUser | null };
type RawUser = Record<string, unknown>;
type RawUserRevocation = Record<string, unknown>;
type RawSession = Record<string, unknown>;
type RawChannelSettingsSyncResponse = {
  platform: string;
  synced: boolean;
  synced_at: number;
  connector: RawPluginStatus;
};

function toPluginStatus(raw: RawPluginStatus): IChannelPluginStatus {
  return {
    id: (raw.plugin_id ?? raw.id) as string,
    type: (raw.type ?? raw.plugin_type) as string,
    name: raw.name as string,
    enabled: raw.enabled as boolean,
    connected: (raw.connected ?? false) as boolean,
    status: raw.status as string | undefined,
    last_connected: raw.last_connected as number | undefined,
    activeUsers: (raw.active_users ?? 0) as number,
    botUsername: raw.bot_username as string | undefined,
    hasToken: (raw.has_token ?? false) as boolean,
    isExtension: raw.is_extension as boolean | undefined,
    extensionMeta: raw.extension_meta as IChannelPluginStatus['extensionMeta'],
  };
}

function toPairing(raw: RawPairing): IChannelPairingRequest {
  return {
    code: raw.code as string,
    platformUserId: raw.platform_user_id as string,
    platformType: raw.platform_type as string,
    display_name: raw.display_name as string | undefined,
    requestedAt: raw.requested_at as number,
    expiresAt: raw.expires_at as number,
  };
}

function toPairingDecision(raw: RawPairingDecision): IChannelPairingDecision {
  return {
    code: raw.code as string,
    status: raw.status as IChannelPairingDecision['status'],
    platformUserId: raw.platform_user_id as string,
    platformType: raw.platform_type as string,
    user: raw.user ? toChannelUser(raw.user) : null,
  };
}

function toChannelUser(raw: RawUser): IChannelUser {
  return {
    id: raw.id as string,
    platformUserId: raw.platform_user_id as string,
    platformType: raw.platform_type as string,
    display_name: raw.display_name as string | undefined,
    authorizedAt: raw.authorized_at as number,
    lastActive: raw.last_active as number | undefined,
    session_id: raw.session_id as string | undefined,
  };
}

function toChannelUserRevocation(raw: RawUserRevocation): IChannelUserRevocation {
  return {
    user_id: raw.user_id as string,
    status: raw.status as IChannelUserRevocation['status'],
  };
}

function toChannelSession(raw: RawSession): IChannelSession {
  return {
    id: raw.id as string,
    user_id: raw.user_id as string,
    agent_type: raw.agent_type as string,
    conversation_id: raw.conversation_id as string | undefined,
    workspace: raw.workspace as string | undefined,
    chatId: raw.chat_id as string | undefined,
    created_at: raw.created_at as number,
    lastActivity: raw.last_activity as number,
  };
}

export const channel = {
  getPluginStatus: withResponseMap(httpGet<RawPluginStatus[], void>('/api/channel/plugins'), (raw) =>
    raw.map(toPluginStatus)
  ),
  enablePlugin: withResponseMap(
    httpPost<RawPluginStatus, { plugin_id: string; config: Record<string, unknown> }>('/api/channel/plugins/enable'),
    toPluginStatus
  ),
  disablePlugin: withResponseMap(
    httpPost<RawPluginStatus, { plugin_id: string }>('/api/channel/plugins/disable'),
    toPluginStatus
  ),
  testPlugin: httpPost<
    { success: boolean; bot_username?: string; error?: string },
    { plugin_id: string; token: string; extra_config?: { app_id?: string; app_secret?: string } }
  >('/api/channel/plugins/test'),
  getPendingPairings: withResponseMap(httpGet<RawPairing[], void>('/api/channel/pairings'), (raw) =>
    raw.map(toPairing)
  ),
  approvePairing: withResponseMap(
    httpPost<RawPairingDecision, { code: string }>('/api/channel/pairings/approve'),
    toPairingDecision
  ),
  rejectPairing: withResponseMap(
    httpPost<RawPairingDecision, { code: string }>('/api/channel/pairings/reject'),
    toPairingDecision
  ),
  getAuthorizedUsers: withResponseMap(httpGet<RawUser[], void>('/api/channel/users'), (raw) => raw.map(toChannelUser)),
  revokeUser: withResponseMap(
    httpPost<RawUserRevocation, { user_id: string }>('/api/channel/users/revoke'),
    toChannelUserRevocation
  ),
  getActiveSessions: withResponseMap(httpGet<RawSession[], void>('/api/channel/sessions'), (raw) =>
    raw.map(toChannelSession)
  ),
  ingressMessage: httpPost<IChannelIngressMessageResponse, IChannelIngressMessageRequest>(
    '/api/channel/ingress/messages'
  ),
  getPlatformSettings: httpGet<IChannelPlatformSettings, { platform: string }>(
    (p) => `/api/channel/settings/${encodeURIComponent(p.platform)}`
  ),
  setAssistantSetting: httpPut<void, { platform: string; assistant: IChannelAssistantBindingWrite }>(
    (p) => `/api/channel/settings/${encodeURIComponent(p.platform)}/assistant`,
    (p) => p.assistant
  ),
  setDefaultModelSetting: httpPut<void, { platform: string; default_model: IChannelDefaultModelSetting }>(
    (p) => `/api/channel/settings/${encodeURIComponent(p.platform)}/default-model`,
    (p) => p.default_model
  ),
  syncChannelSettings: withResponseMap(
    httpPost<RawChannelSettingsSyncResponse, { platform: string }>('/api/channel/settings/sync'),
    (raw): IChannelSettingsSyncResponse => ({
      platform: raw.platform,
      synced: raw.synced,
      synced_at: raw.synced_at,
      connector: toPluginStatus(raw.connector),
    })
  ),
  pairingRequested: wsMappedEmitter<IChannelPairingRequest>('channel.pairing-requested', (raw) =>
    toPairing(raw as RawPairing)
  ),
  pluginStatusChanged: wsMappedEmitter<{ plugin_id: string; status: IChannelPluginStatus }>(
    'channel.plugin-status-changed',
    (raw) => {
      const r = raw as Record<string, unknown>;
      return {
        plugin_id: r.plugin_id as string,
        status: toPluginStatus(r.status as RawPluginStatus),
      };
    }
  ),
  userAuthorized: wsMappedEmitter<IChannelUser>('channel.user-authorized', (raw) => toChannelUser(raw as RawUser)),
};
