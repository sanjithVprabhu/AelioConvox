import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import {
  AgentCompletionRequestSchema,
  AgentCompletionResponseSchema,
  AgentGatewayCapabilitiesSchema,
  extractJsonObject,
  formatReactToolCatalog,
  parseReactAssistantText,
  reactRequestToLlmOptions,
} from '../server/src/routes/agent-gateway-contract.ts';

const fixture = async (name: string): Promise<unknown> =>
  JSON.parse(await readFile(resolve('docs/harness_new/fixtures', name), 'utf8'));

async function main() {
  const request = AgentCompletionRequestSchema.parse(
    await fixture('model_gateway_v2_request.json'),
  );
  const response = AgentCompletionResponseSchema.parse(
    await fixture('model_gateway_v2_response.json'),
  );

  assert.equal(request.protocol_version, 2);
  assert.equal(response.request_id, request.request_id);
  assert.equal(response.attempt_id, request.attempt_id);

  const options = reactRequestToLlmOptions(request);
  assert.equal(options.telemetry.purpose, 'agent_loop');
  assert.equal(options.tools.length, 0, 'ReAct path must not attach provider tools');
  assert.equal(options.responseFormat?.type, 'json_object');
  assert.equal('toolChoice' in options, false);
  assert.match(options.system, /Available tools/);
  assert.match(options.system, /finish/);
  assert.equal(options.messages[0]?.role, 'user');
  for (const message of options.messages) {
    assert.equal('toolCalls' in message, false);
    assert.equal('toolResults' in message, false);
  }

  assert.equal(
    AgentCompletionRequestSchema.safeParse({ ...request, unexpected: true }).success,
    false,
    'the TypeScript boundary must reject unknown fields',
  );

  const capabilities = AgentGatewayCapabilitiesSchema.parse({
    protocol_version: 2,
    provider: 'mock',
    model: 'mock-model',
    native_tools: false,
    tool_transport: 'react_json',
    prompt_caching: false,
    streaming: false,
    max_output_tokens: 4096,
  });
  assert.equal(capabilities.native_tools, false);
  assert.equal(capabilities.tool_transport, 'react_json');
  assert.equal(
    AgentGatewayCapabilitiesSchema.safeParse({
      ...capabilities,
      native_tools: true,
    }).success,
    false,
  );
  assert.equal(
    AgentGatewayCapabilitiesSchema.safeParse({ ...capabilities, unexpected: true }).success,
    false,
  );

  // Parse: raw JSON finish
  const finishParsed = parseReactAssistantText(
    JSON.stringify({
      thought: 'done',
      actions: [{
        name: 'finish',
        arguments: {
          message: 'Hello',
          status: 'completed',
          resolved_effect_ids: [],
          unresolved_effect_ids: [],
        },
      }],
    }),
    'req-1',
  );
  assert.equal(finishParsed.stop_reason, 'tool_use');
  assert.equal(finishParsed.content.length, 2);
  assert.equal(finishParsed.content[0]?.type, 'text');
  assert.equal(finishParsed.content[1]?.type, 'tool_call');
  if (finishParsed.content[1]?.type === 'tool_call') {
    assert.equal(finishParsed.content[1].call.id, 'react-req-1-1');
    assert.equal(finishParsed.content[1].call.name, 'finish');
  }

  // Parse: fenced JSON multi-action
  const fenced = parseReactAssistantText(
    '```json\n{"thought":"x","actions":[{"name":"a","arguments":{"k":1}},{"name":"b","arguments":{}}]}\n```',
    'req-2',
  );
  assert.equal(fenced.content.filter((block) => block.type === 'tool_call').length, 2);

  // Parse: invalid → text only
  const invalid = parseReactAssistantText('sorry I cannot help', 'req-3');
  assert.equal(invalid.stop_reason, 'end_turn');
  assert.equal(invalid.content[0]?.type, 'text');

  assert.deepEqual(extractJsonObject('prefix {"a":1} suffix'), { a: 1 });
  assert.match(formatReactToolCatalog(request.tools), /input_schema/);

  // History encoding: tool_result becomes Observation text
  const withObservation = reactRequestToLlmOptions({
    ...request,
    messages: [
      ...request.messages,
      {
        role: 'tool_result',
        result: {
          call_id: 'c1',
          tool: 'finish',
          data: { accepted: true },
          error: null,
        },
      },
    ],
  });
  const observation = withObservation.messages.at(-1);
  assert.equal(observation?.role, 'user');
  assert.match(observation?.content ?? '', /^Observation:/);

  console.log('agent-loop gateway v2 ReAct contract: PASS');
}

void main();
