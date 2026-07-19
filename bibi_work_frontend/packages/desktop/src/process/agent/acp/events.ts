import type { SessionNotification, SessionUpdate, StopReason } from '@agentclientprotocol/sdk';

export type PlatformRunEvent = {
  event_id: string;
  type: string;
  payload: Record<string, unknown>;
};

export type AcpEventMapper = {
  map: (notification: SessionNotification) => PlatformRunEvent[];
  finish: (stopReason: StopReason) => PlatformRunEvent[];
};

function textFromUpdate(update: SessionUpdate): string | null {
  if (!('content' in update) || Array.isArray(update.content)) return null;
  const content = update.content as { type?: unknown; text?: unknown };
  return content.type === 'text' && typeof content.text === 'string' ? content.text : null;
}

export function createAcpEventMapper(runId: string): AcpEventMapper {
  let sequence = 0;
  let messageStarted = false;
  let thinkingStarted = false;
  let message = '';
  let thinking = '';
  const event = (type: string, payload: Record<string, unknown>): PlatformRunEvent => ({
    event_id: `desktop-acp.${runId}.${++sequence}`,
    type,
    payload,
  });

  return {
    map(notification) {
      const update = notification.update;
      if (update.sessionUpdate === 'agent_message_chunk') {
        const content = textFromUpdate(update);
        if (content === null) return [];
        const events: PlatformRunEvent[] = [];
        if (!messageStarted) {
          messageStarted = true;
          events.push(event('message.started', { role: 'assistant' }));
        }
        message += content;
        events.push(event('message.delta', { role: 'assistant', content }));
        return events;
      }
      if (update.sessionUpdate === 'agent_thought_chunk') {
        const content = textFromUpdate(update);
        if (content === null) return [];
        const events: PlatformRunEvent[] = [];
        if (!thinkingStarted) {
          thinkingStarted = true;
          events.push(event('thinking.started', {}));
        }
        thinking += content;
        events.push(event('thinking.delta', { content }));
        return events;
      }
      if (update.sessionUpdate === 'tool_call') {
        const status = update.status ?? 'pending';
        return [
          event(status === 'pending' ? 'tool.call.requested' : 'tool.call.started', {
            tool_call_id: update.toolCallId,
            tool_name: update.title,
            title: update.title,
            kind: update.kind ?? 'other',
            status,
            arguments: update.rawInput ?? {},
          }),
        ];
      }
      if (update.sessionUpdate === 'tool_call_update') {
        const status = update.status ?? 'in_progress';
        const type =
          status === 'completed' ? 'tool.call.completed' : status === 'failed' ? 'tool.call.failed' : 'tool.call.delta';
        return [
          event(type, {
            tool_call_id: update.toolCallId,
            title: update.title,
            kind: update.kind,
            status,
            output: update.rawOutput,
            content: update.content,
          }),
        ];
      }
      return [event('activity.raw', { source: 'acp', update })];
    },
    finish(stopReason) {
      const events: PlatformRunEvent[] = [];
      if (thinkingStarted) events.push(event('thinking.completed', { content: thinking }));
      if (messageStarted) events.push(event('message.completed', { role: 'assistant', content: message }));
      if (stopReason === 'cancelled') {
        events.push(event('run.cancelled', { reason: 'acp_cancelled' }));
      } else if (stopReason === 'refusal') {
        events.push(event('run.failed', { error: 'ACP agent refused the prompt', stop_reason: stopReason }));
      } else {
        events.push(event('run.completed', { stop_reason: stopReason }));
      }
      return events;
    },
  };
}
