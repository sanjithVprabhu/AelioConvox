import assert from 'node:assert/strict';
import { MockProvider } from '../packages/llm/src/providers/mock.ts';
import type { ChatMessage, ToolDefinition } from '../packages/llm/src/types.ts';

const finish: ToolDefinition = {
  name: 'finish',
  description: 'Finish with an explicit customer-facing status.',
  input_schema: { type: 'object' },
};
const listOrders: ToolDefinition = {
  name: 'list_orders',
  description: 'List the customer orders.',
  input_schema: { type: 'object' },
};
const memorySearch: ToolDefinition = {
  name: 'memory_search',
  description: 'Search the current customer conversation memory.',
  input_schema: { type: 'object' },
};

async function main() {
  const provider = new MockProvider();
  const messages: ChatMessage[] = [{ role: 'user', content: 'list my orders' }];
  const first = await provider.complete({
    messages,
    tools: [finish, listOrders],
    model: 'mock-model',
  });
  assert.equal(first.toolCalls[0]?.name, 'list_orders');

  messages.push({ role: 'assistant', content: '', toolCalls: first.toolCalls });
  messages.push({
    role: 'user',
    content: '',
    toolResults: [{
      toolUseId: first.toolCalls[0]!.id,
      content: JSON.stringify({ data: { count: 3 }, error: null }),
    }],
  });
  const second = await provider.complete({
    messages,
    tools: [finish, listOrders],
    model: 'mock-model',
  });
  assert.equal(second.toolCalls[0]?.name, 'finish');
  assert.match(String(second.toolCalls[0]?.args.message), /3 orders/);

  messages.push({ role: 'assistant', content: '', toolCalls: second.toolCalls });
  messages.push({ role: 'user', content: 'yes' });
  const nextTurn = await provider.complete({
    messages,
    tools: [finish, listOrders],
    model: 'mock-model',
  });
  assert.equal(nextTurn.toolCalls[0]?.name, 'finish');
  assert.notEqual(nextTurn.toolCalls[0]?.name, 'list_orders');

  const memoryMessages: ChatMessage[] = [
    { role: 'user', content: 'what units do I prefer?' },
  ];
  const memoryCall = await provider.complete({
    messages: memoryMessages,
    tools: [finish, memorySearch],
    model: 'mock-model',
  });
  assert.equal(memoryCall.toolCalls[0]?.name, 'memory_search');
  memoryMessages.push({ role: 'assistant', content: '', toolCalls: memoryCall.toolCalls });
  memoryMessages.push({
    role: 'user',
    content: '',
    toolResults: [{
      toolUseId: memoryCall.toolCalls[0]!.id,
      content: JSON.stringify({
        tool: 'memory_search',
        data: { results: [{ content: 'Customer prefers metric units.' }] },
        error: null,
      }),
    }],
  });
  const memoryReply = await provider.complete({
    messages: memoryMessages,
    tools: [finish, memorySearch],
    model: 'mock-model',
  });
  assert.equal(memoryReply.toolCalls[0]?.name, 'finish');
  assert.match(String(memoryReply.toolCalls[0]?.args.message), /metric units/i);

  console.log('agent-loop deterministic provider sequencing: PASS');
}

void main();
