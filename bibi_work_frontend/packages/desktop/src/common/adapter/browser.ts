/**
 * @license
 * Copyright 2025 AionUi (aionui.com)
 * SPDX-License-Identifier: Apache-2.0
 */

import { bridge, logger } from '@office-ai/platform';
import { WEBUI_DEFAULT_PORT } from '@/common/config/constants';
import { clearAccessToken, getAccessToken, peekAccessToken, subscribeAccessToken } from '@/common/auth/authTokenBroker';
import type { ElectronBridgeAPI } from '@/common/types/platform/electron';

interface CustomWindow extends Window {
  electronAPI?: ElectronBridgeAPI;
  __bridgeEmitter?: { emit: (name: string, data: unknown) => void };
  __emitBridgeCallback?: (name: string, data: unknown) => void;
  __websocketReconnect?: () => void;
}

type BrowserWebSocketPayload = { name: string; data?: unknown };
type EnterpriseSubscriptionScope = 'conversation' | 'team' | 'cron' | 'channel' | 'extensions' | 'hub';

const ENTERPRISE_SUBSCRIPTION_SCOPES: EnterpriseSubscriptionScope[] = [
  'conversation',
  'team',
  'cron',
  'channel',
  'extensions',
  'hub',
];

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

function isBrowserWebSocketPayload(value: unknown): value is BrowserWebSocketPayload {
  return isRecord(value) && typeof value.name === 'string';
}

export function isRealtimeAuthTerminalError(payload: unknown): boolean {
  if (isBrowserWebSocketPayload(payload) && (payload.name === 'auth.revoked' || payload.name === 'auth.failed')) {
    return true;
  }

  const data = getRealtimeErrorData(payload);
  if (!data) {
    return false;
  }

  const { code } = data;
  return code === 'REALTIME_AUTH_MISSING' || code === 'REALTIME_AUTH_EXPIRED';
}

function getRealtimeErrorData(payload: unknown): Record<string, unknown> | null {
  if (!isBrowserWebSocketPayload(payload) || payload.name !== 'realtime.error' || !isRecord(payload.data)) {
    return null;
  }

  return payload.data;
}

function isUnrecoverableRealtimeError(payload: unknown): boolean {
  return getRealtimeErrorData(payload)?.recoverable === false;
}

const win = window as CustomWindow;

/**
 * 适配electron的API到浏览器中,建立renderer和main的通信桥梁, 与preload.ts中的注入对应
 * */
if (win.electronAPI) {
  // Electron 环境 - 使用 IPC 通信
  bridge.adapter({
    emit(name, data) {
      return win.electronAPI.emit(name, data);
    },
    on(emitter) {
      win.electronAPI?.on((event) => {
        try {
          const { value } = event;
          const { name, data } = JSON.parse(value);
          emitter.emit(name, data);
        } catch (e) {
          console.warn('JSON parsing error:', e);
        }
      });
    },
  });
} else {
  // Web 环境 - 使用 WebSocket 通信，并在登录后用共享 token 完成首帧认证。
  // Web runtime bridge: connect only after login so the first frame can be auth.
  // Path must be `/ws` — web-host's static-server only proxies WebSocket upgrades under /ws.
  const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
  const defaultHost = `${window.location.hostname}:${WEBUI_DEFAULT_PORT}`;
  const socketUrl = `${protocol}//${window.location.host || defaultHost}/ws`;

  type QueuedMessage = { name: string; data: unknown };

  let socket: WebSocket | null = null;
  let emitterRef: { emit: (name: string, data: unknown) => void } | null = null;
  let reconnectTimer: number | null = null;
  let reconnectDelay = 500;
  let shouldReconnect = true; // Flag to control reconnection
  let socketAuthenticated = false;

  const messageQueue: QueuedMessage[] = [];

  // 1.发送队列中积压的消息，确保在重新建立连接后不会丢事件
  const flushQueue = () => {
    if (!socket || socket.readyState !== WebSocket.OPEN || !socketAuthenticated) {
      return;
    }

    while (messageQueue.length > 0) {
      const queued = messageQueue.shift();
      if (queued) {
        socket.send(JSON.stringify(queued));
      }
    }
  };

  // 2.简单的指数退避重连，等待服务端在登录成功后接受新连接
  const scheduleReconnect = () => {
    if (reconnectTimer !== null || !shouldReconnect) {
      return;
    }

    reconnectTimer = window.setTimeout(() => {
      reconnectTimer = null;
      reconnectDelay = Math.min(reconnectDelay * 2, 8000);
      connect();
    }, reconnectDelay);
  };

  const clearReconnectTimer = () => {
    if (reconnectTimer === null) {
      return;
    }
    window.clearTimeout(reconnectTimer);
    reconnectTimer = null;
  };

  const closeSocketWithoutReconnect = () => {
    shouldReconnect = false;
    socketAuthenticated = false;
    clearReconnectTimer();
    const currentSocket = socket;
    socket = null;
    currentSocket?.close();
  };

  const authenticateOpenSocket = (token: string) => {
    if (!socket || socket.readyState !== WebSocket.OPEN) {
      return false;
    }
    socket.send(JSON.stringify({ op: 'auth', access_token: token }));
    socketAuthenticated = true;
    sendEnterpriseSubscriptions(socket);
    flushQueue();
    return true;
  };

  const sendEnterpriseSubscriptions = (targetSocket: WebSocket) => {
    for (const scope of ENTERPRISE_SUBSCRIPTION_SCOPES) {
      targetSocket.send(JSON.stringify({ op: 'subscribe', scope }));
    }
  };

  // 3.建立 WebSocket 连接（或复用已有的 OPEN/CONNECTING 状态）
  const connect = () => {
    if (!peekAccessToken()) {
      return;
    }
    if (socket && (socket.readyState === WebSocket.OPEN || socket.readyState === WebSocket.CONNECTING)) {
      return;
    }

    try {
      socket = new WebSocket(socketUrl);
      socketAuthenticated = false;
    } catch {
      scheduleReconnect();
      return;
    }

    // Capture the socket created in this call so the close handler only
    // nulls the outer reference when it still points at THIS socket.
    // Without this guard, a late-firing close event from the OLD socket
    // could wipe the reference to a NEWLY created replacement socket.
    const currentSocket = socket;

    currentSocket.addEventListener('open', () => {
      reconnectDelay = 500;
      void getAccessToken()
        .then((token) => {
          if (token && currentSocket.readyState === WebSocket.OPEN) {
            currentSocket.send(JSON.stringify({ op: 'auth', access_token: token }));
            socketAuthenticated = true;
            sendEnterpriseSubscriptions(currentSocket);
            flushQueue();
          } else if (socket === currentSocket) {
            closeSocketWithoutReconnect();
          }
        })
        .catch(() => {
          if (socket === currentSocket) {
            closeSocketWithoutReconnect();
          }
        });
    });

    currentSocket.addEventListener('message', (event: MessageEvent) => {
      try {
        const payload = JSON.parse(event.data as string) as unknown;

        if (!isBrowserWebSocketPayload(payload)) {
          return;
        }

        // 处理服务端心跳 ping，立即回复 pong 以保持连接
        // Handle server heartbeat ping - respond with pong immediately to keep connection alive
        if (payload.name === 'ping') {
          if (socket && socket.readyState === WebSocket.OPEN) {
            socket.send(JSON.stringify({ name: 'pong', data: { timestamp: Date.now() } }));
          }
          return;
        }

        // 处理认证过期 - 停止重连并跳转到登录页
        // Handle auth expiration - stop reconnecting and redirect to login
        if (isRealtimeAuthTerminalError(payload)) {
          console.warn('[WebSocket] Authentication expired, stopping reconnection');
          shouldReconnect = false;
          socketAuthenticated = false;
          clearAccessToken();

          // 清除所有待执行的重连定时器
          // Clear any pending reconnection timer
          if (reconnectTimer !== null) {
            window.clearTimeout(reconnectTimer);
            reconnectTimer = null;
          }

          // 关闭 socket 并跳转到登录页
          // Close the socket and redirect to login page
          if (currentSocket.readyState !== WebSocket.CLOSED && currentSocket.readyState !== WebSocket.CLOSING) {
            currentSocket.close();
          }

          // 已在登录页则不再重定向，防止无限刷新循环
          // Skip redirect if already on login page to prevent infinite reload loop
          if (window.location.pathname === '/login' || window.location.hash.includes('/login')) {
            return;
          }

          // 短暂延迟后跳转到登录页，以便显示 UI 反馈
          // Redirect to login page after a short delay to show any UI feedback
          // Use hash navigation to stay within the SPA (HashRouter), avoiding a full
          // page reload that would land on an empty hash and cause a blank screen.
          setTimeout(() => {
            window.location.hash = '/login';
          }, 1000);

          return;
        }

        if (isUnrecoverableRealtimeError(payload)) {
          console.warn('[WebSocket] Unrecoverable realtime error, reconnecting');
          emitterRef?.emit(payload.name, payload.data);
          currentSocket.close();
          return;
        }

        if (!emitterRef) {
          return;
        }

        emitterRef.emit(payload.name, payload.data);
      } catch {
        // 忽略格式错误的消息 / Ignore malformed payloads
      }
    });

    currentSocket.addEventListener('close', () => {
      // Only null the outer reference if it still points at this socket.
      if (socket === currentSocket) {
        socket = null;
        socketAuthenticated = false;
      }

      scheduleReconnect();
    });

    currentSocket.addEventListener('error', () => {
      currentSocket.close();
    });
  };

  // 4.确保在发送/订阅前已经发起连接
  const ensureSocket = () => {
    if (!peekAccessToken()) {
      return;
    }
    if (!socket || socket.readyState === WebSocket.CLOSED || socket.readyState === WebSocket.CLOSING) {
      connect();
    }
  };

  bridge.adapter({
    emit(name, data) {
      const message: QueuedMessage = { name, data };

      ensureSocket();

      if (socket && socket.readyState === WebSocket.OPEN && socketAuthenticated) {
        try {
          socket.send(JSON.stringify(message));
          return;
        } catch {
          scheduleReconnect();
        }
      }

      messageQueue.push(message);
    },
    on(emitter) {
      emitterRef = emitter;
      win.__bridgeEmitter = emitter;

      // Expose callback emitter for bridge provider pattern
      // Used by components to send responses back through WebSocket
      win.__emitBridgeCallback = (name: string, data: unknown) => {
        emitter.emit(name, data);
      };

      ensureSocket();
    },
  });

  if (peekAccessToken()) {
    connect();
  }

  // Expose reconnection control for login flow
  win.__websocketReconnect = () => {
    shouldReconnect = true;
    reconnectDelay = 500;
    connect();
  };

  subscribeAccessToken((token) => {
    if (!token) {
      closeSocketWithoutReconnect();
      return;
    }
    shouldReconnect = true;
    reconnectDelay = 500;
    if (!authenticateOpenSocket(token)) {
      connect();
    }
  });
}

logger.provider({
  log(log) {
    console.log('process.log', log.type, ...log.logs);
  },
  path() {
    return Promise.resolve('');
  },
});
