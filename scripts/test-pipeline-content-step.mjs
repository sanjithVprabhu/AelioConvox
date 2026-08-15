/**
 * Verify attribute/content pipeline steps bypass harness binding.
 * Run: node --import tsx scripts/test-pipeline-content-step.mjs
 */
import { createDatabase } from '@aelio/db';
import { evaluatePipeline, shouldUsePipelineConversation } from '@aelio/core';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { randomUUID } from 'node:crypto';

const __dirname = dirname(fileURLToPath(import.meta.url));
const dbPath = join('/tmp', `aelio-pitch-${randomUUID()}.sqlite`);
const database = createDatabase(dbPath);
database.migrate(join(__dirname, '../packages/db/drizzle'));

const manifest = {
  initial_stage: 'unverified',
  stages: {
    unverified: { id: 'unverified', description: 'New visitor', flow: 'welcome', next: 'verified' },
    verified: { id: 'verified', description: 'Verify' },
  },
};

const flows = [
  {
    id: 'welcome',
    state: 'unverified',
    description: 'Welcome',
    steps: [
      { id: 'pitch', goal: 'Deliver greeting', type: 'content' },
      { id: 'intent_ack', goal: 'Confirm intent', type: 'attribute', attribute: 'intent_ack' },
    ],
  },
];

const { customers } = await import('@aelio/db');
const customerId = randomUUID();
const now = new Date();
await database.db.insert(customers).values({
  id: customerId,
  externalId: 'u1',
  displayName: 'u1',
  createdAt: now,
  updatedAt: now,
  metadata: {},
});

const ctx = await evaluatePipeline({
  database,
  internalCustomerId: customerId,
  externalId: 'u1',
  userMessage: 'hiii',
  manifest,
  flows,
  attributes: [
    {
      id: 'intent_ack',
      label: 'Intent',
      data_type: 'string',
      sensitivity_tier: 'public',
      enum_values: ["Let's go"],
    },
  ],
  authenticated: false,
});

if (!ctx.enabled || ctx.currentStep?.id !== 'pitch') {
  console.error('FAIL: expected pitch step, got', ctx.currentStep);
  process.exit(1);
}

if (!shouldUsePipelineConversation(ctx, 'hiii')) {
  console.error('FAIL: pitch content step should bypass harness');
  process.exit(1);
}

console.log('PASS: content step pitch routes away from harness');
database.close();
