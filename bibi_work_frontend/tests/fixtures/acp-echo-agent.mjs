import * as acp from '@agentclientprotocol/sdk';
import { Readable, Writable } from 'node:stream';

const stream = acp.ndJsonStream(Writable.toWeb(process.stdout), Readable.toWeb(process.stdin));
let cancelPrompt = null;

new acp.AgentSideConnection(
  (connection) => ({
    initialize: async (request) => ({
      protocolVersion: request.protocolVersion,
      agentCapabilities: {},
      agentInfo: { name: 'Bibi ACP Echo Fixture', version: '1' },
      authMethods: [],
    }),
    newSession: async () => ({ sessionId: 'fixture-session' }),
    prompt: async (request) => {
      const text = request.prompt.find((block) => block.type === 'text')?.text ?? '';
      if (text === 'wait-for-cancel') {
        await new Promise((resolve) => {
          cancelPrompt = resolve;
        });
        return { stopReason: 'cancelled' };
      }
      if (text === 'request-permission') {
        const permission = await connection.requestPermission({
          sessionId: request.sessionId,
          toolCall: {
            toolCallId: 'fixture-sensitive-tool',
            title: 'Fixture sensitive tool',
            kind: 'execute',
            status: 'pending',
            rawInput: { operation: 'fixture-only' },
          },
          options: [
            { optionId: 'allow-fixture', name: 'Allow once', kind: 'allow_once' },
            { optionId: 'reject-fixture', name: 'Reject', kind: 'reject_once' },
          ],
        });
        const result = permission.outcome.outcome === 'selected' ? permission.outcome.optionId : 'cancelled';
        await connection.sessionUpdate({
          sessionId: request.sessionId,
          update: {
            sessionUpdate: 'agent_message_chunk',
            content: { type: 'text', text: `permission:${result}` },
          },
        });
        return { stopReason: 'end_turn' };
      }
      if (text === 'echo-traceparent') {
        await connection.sessionUpdate({
          sessionId: request.sessionId,
          update: {
            sessionUpdate: 'agent_message_chunk',
            content: { type: 'text', text: `traceparent:${process.env.TRACEPARENT ?? ''}` },
          },
        });
        return { stopReason: 'end_turn' };
      }
      await connection.sessionUpdate({
        sessionId: request.sessionId,
        update: {
          sessionUpdate: 'agent_message_chunk',
          content: { type: 'text', text: `echo:${text}` },
        },
      });
      return { stopReason: 'end_turn' };
    },
    cancel: async () => {
      cancelPrompt?.();
      cancelPrompt = null;
    },
  }),
  stream
);
