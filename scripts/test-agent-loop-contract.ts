import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import {
  AgentCompletionRequestSchema,
  AgentCompletionResponseSchema,
  AgentGatewayCapabilitiesSchema,
  agentRequestToLlmOptions,
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
  const options = agentRequestToLlmOptions(request);
  assert.equal(options.telemetry.purpose, 'agent_loop');
  assert.equal(options.tools[0]?.name, 'finish');
  assert.equal(options.messages[1]?.role, 'user');

  assert.equal(
    AgentCompletionRequestSchema.safeParse({ ...request, unexpected: true }).success,
    false,
    'the TypeScript boundary must reject unknown fields',
  );

  const capabilities = AgentGatewayCapabilitiesSchema.parse({
    protocol_version: 2,
    provider: 'mock',
    model: 'mock-model',
    native_tools: true,
    prompt_caching: false,
    streaming: false,
    max_output_tokens: 4096,
  });
  assert.equal(capabilities.native_tools, true);
  assert.equal(
    AgentGatewayCapabilitiesSchema.safeParse({ ...capabilities, unexpected: true }).success,
    false,
  );

  console.log('agent-loop gateway v2 contract: PASS');
}

void main();
