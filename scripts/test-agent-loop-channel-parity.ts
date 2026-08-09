import assert from 'node:assert/strict';
import { executeConversationTurn } from '../server/src/conversation-turn.ts';
import type { RuntimeDeps } from '../server/src/runtime-deps.ts';

type CapturedTurn = {
  turn_id: string;
  user_id: string;
  utterance: string;
  channel: string;
};

const captured: CapturedTurn[] = [];
const deps = {
  aelioRuntime: {
    async submitAgentTurn(turn: CapturedTurn) {
      captured.push(turn);
      return {
        reply: { text: 'The order is ready.', frame: null },
        tier: 'agent_loop',
        llm_calls: 2,
        steps: [
          {
            name: 'Harness.Mode',
            detail: 'mode=agent_loop legacy_spine=false reason=AELIO_HARNESS_MODE=agent_loop',
          },
          {
            name: 'AgentLoop.Run',
            detail: 'turns=2 tool_calls=1 events=4 manifest_hash=test fence=1',
          },
        ],
        suspended: false,
        opened_loop: false,
      };
    },
  },
} as unknown as RuntimeDeps;

async function main() {
  const common = {
    customerExternalId: '+15550001111',
    channelAddress: '+15550001111',
    message: 'Where is my order?',
    sourceTurnId: 'provider-message-1',
  };
  const web = await executeConversationTurn(deps, { ...common, channel: 'web' });
  const whatsapp = await executeConversationTurn(deps, { ...common, channel: 'whatsapp' });

  assert.equal(captured.length, 2);
  assert.equal(captured[0]?.user_id, captured[1]?.user_id, 'identity must be channel-independent');
  assert.equal(captured[0]?.utterance, captured[1]?.utterance);
  assert.equal(captured[0]?.channel, 'web');
  assert.equal(captured[1]?.channel, 'whatsapp');
  assert.notEqual(captured[0]?.turn_id, captured[1]?.turn_id, 'ingress dedup keys include channel');
  assert.equal(web.reply, whatsapp.reply);
  assert.equal(web.awaitingConfirmation, whatsapp.awaitingConfirmation);
  assert.equal(web.frame.blocks[0]?.type, whatsapp.frame.blocks[0]?.type);

  console.log('agent-loop web/WhatsApp ingress parity: PASS');
}

void main();
