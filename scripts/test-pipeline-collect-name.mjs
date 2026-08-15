/**
 * Verify collect_name attribute step bypasses harness.
 * Run: node --import tsx scripts/test-pipeline-collect-name.mjs
 */
import { createDatabase } from '@aelio/db';
import {
  evaluatePipeline,
  patchPipelineContext,
  resolvePipelineRoute,
} from '@aelio/core';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { randomUUID } from 'node:crypto';

const __dirname = dirname(fileURLToPath(import.meta.url));
const dbPath = join('/tmp', `aelio-collect-name-${randomUUID()}.sqlite`);
const database = createDatabase(dbPath);
database.migrate(join(__dirname, '../packages/db/drizzle'));

const manifest = {
  initial_stage: 'onboarding',
  stages: {
    onboarding: {
      id: 'onboarding',
      description: 'Setup',
      flow: 'shopco_onboarding',
      next: 'active',
    },
    active: { id: 'active', description: 'Done' },
  },
};

const flows = [
  {
    id: 'shopco_onboarding',
    state: 'onboarding',
    description: 'Onboarding',
    steps: [
      {
        id: 'collect_name',
        goal: 'Ask for their name',
        type: 'attribute',
        attribute: 'display_name',
      },
      {
        id: 'sync_profile',
        goal: 'Save profile',
        type: 'tool',
        tool: 'syncProfile',
      },
    ],
  },
];

const { customers } = await import('@aelio/db');
const customerId = randomUUID();
const now = new Date();
await database.db.insert(customers).values({
  id: customerId,
  externalId: 'u-collect-name',
  displayName: 'u-collect-name',
  createdAt: now,
  updatedAt: now,
  metadata: { lifecycleState: 'onboarding' },
});

const { upsertPipelineState } = await import('@aelio/core');
await upsertPipelineState(database.db, customerId, 'onboarding');

const ctx = patchPipelineContext(
  await evaluatePipeline({
    database,
    internalCustomerId: customerId,
    externalId: 'u-collect-name',
    userMessage: 'Rishabh',
    manifest,
    flows,
    attributes: [
      {
        id: 'display_name',
        label: 'Name',
        data_type: 'string',
        sensitivity_tier: 'pii',
        prompts: ['What should I call you?'],
      },
    ],
    authenticated: false,
  }),
  flows,
);

if (ctx.currentStep?.id !== 'collect_name') {
  console.error('FAIL: expected collect_name step, got', ctx.currentStep?.id);
  process.exit(1);
}

if (resolvePipelineRoute(ctx, 'Rishabh') !== 'conversation') {
  console.error('FAIL: collect_name should route to conversation, not harness');
  process.exit(1);
}

console.log('PASS: collect_name routes away from harness');
database.close();
