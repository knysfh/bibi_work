import { describe, expect, it } from 'vitest';
import { createAcpEventMapper } from '../../../../../packages/desktop/src/process/agent/acp/events';

describe('ACP event mapper', () => {
  it('normalizes streamed assistant text and terminal state', () => {
    const mapper = createAcpEventMapper('run-1');
    const streamed = mapper.map({
      sessionId: 'session-1',
      update: { sessionUpdate: 'agent_message_chunk', content: { type: 'text', text: 'hello' } },
    });
    expect(streamed.map((event) => event.type)).toEqual(['message.started', 'message.delta']);
    expect(mapper.finish('end_turn').map((event) => event.type)).toEqual(['message.completed', 'run.completed']);
  });

  it('maps tool failures with the platform tool_call_id contract', () => {
    const mapper = createAcpEventMapper('run-2');
    const events = mapper.map({
      sessionId: 'session-2',
      update: { sessionUpdate: 'tool_call_update', toolCallId: 'tool-1', status: 'failed', rawOutput: 'boom' },
    });
    expect(events).toMatchObject([{ type: 'tool.call.failed', payload: { tool_call_id: 'tool-1', status: 'failed' } }]);
  });

  it('fails closed on ACP refusal', () => {
    const mapper = createAcpEventMapper('run-3');
    expect(mapper.finish('refusal')).toMatchObject([{ type: 'run.failed' }]);
  });
});
