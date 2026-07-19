export interface IChannelPluginStatus {
  id: string;
  type: string;
  name: string;
  enabled: boolean;
  connected: boolean;
  status?: string;
  last_connected?: number;
  error?: string;
  activeUsers: number;
  botUsername?: string;
  hasToken?: boolean;
  isExtension?: boolean;
  extensionMeta?: {
    credentialFields?: Array<{
      key: string;
      label: string;
      type: 'text' | 'password' | 'select' | 'number' | 'boolean';
      required?: boolean;
      options?: string[];
      default?: string | number | boolean;
    }>;
    configFields?: Array<{
      key: string;
      label: string;
      type: 'text' | 'password' | 'select' | 'number' | 'boolean';
      required?: boolean;
      options?: string[];
      default?: string | number | boolean;
    }>;
    description?: string;
    extensionName?: string;
    icon?: string;
  };
}

export interface IChannelPairingRequest {
  code: string;
  platformUserId: string;
  platformType: string;
  display_name?: string;
  requestedAt: number;
  expiresAt: number;
}

export interface IChannelPairingDecision {
  code: string;
  status: 'approved' | 'rejected';
  platformUserId: string;
  platformType: string;
  user?: IChannelUser | null;
}

export interface IChannelUser {
  id: string;
  platformUserId: string;
  platformType: string;
  display_name?: string;
  authorizedAt: number;
  lastActive?: number;
  session_id?: string;
}

export interface IChannelUserRevocation {
  user_id: string;
  status: 'revoked';
}

export interface IChannelSession {
  id: string;
  user_id: string;
  agent_type: string;
  conversation_id?: string;
  workspace?: string;
  chatId?: string;
  created_at: number;
  lastActivity: number;
}

export interface IChannelIngressMessageRequest {
  platform?: string;
  platform_type?: string;
  platform_user_id: string;
  chat_id?: string;
  content?: unknown;
  message?: unknown;
  text?: unknown;
  message_id?: string;
  external_message_id?: string;
  files?: unknown[];
  metadata?: Record<string, unknown>;
}

export interface IChannelIngressMessageResponse {
  session_id: string;
  conversation_id: string;
  run_id: string;
  created_conversation: boolean;
}

/**
 * Channel assistant binding shape returned by existing backend/config records.
 * Legacy rows may still carry `custom_agent_id`, `backend`, or `agent_type`;
 * new writes must use {@link IChannelAssistantBindingWrite} instead.
 */
export interface IChannelAssistantBindingRead {
  assistant_id?: string;
  /** @deprecated Legacy assistant identity written before assistant-first migration. */
  custom_agent_id?: string;
  /** @deprecated Legacy backend-only binding kept for read compatibility. */
  backend?: string;
  /** @deprecated Legacy conversation type / backend marker kept for read compatibility. */
  agent_type?: string;
  name?: string;
}

export interface IChannelAssistantBindingWrite {
  assistant_id: string;
}

export interface IChannelDefaultModelSetting {
  model_profile_id?: string;
  id: string;
  use_model: string;
}

export interface IChannelPlatformSettings {
  platform: string;
  assistant: IChannelAssistantBindingRead | null;
  default_model: IChannelDefaultModelSetting | null;
}

export interface IChannelSettingsSyncResponse {
  platform: string;
  synced: boolean;
  synced_at: number;
  connector: IChannelPluginStatus;
}
