/**
 * Smoke test for the pipeline state engine.
 * Run: node --import tsx scripts/test-pipeline-phase1.mjs
 */
import { createDatabase } from '@aelio/db';
import { evaluatePipeline, upsertPipelineState } from '@aelio/core';
import { mkdirSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { tmpdir } from 'node:os';
import { randomUUID } from 'node:crypto';
import { fileURLToPath } from 'node:url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const dbPath = join(tmpdir(), `aelio-pipeline-test-${randomUUID()}.sqlite`);
mkdirSync(join(dbPath, '..'), { recursive: true });
const database = createDatabase(dbPath);
database.migrate(join(__dirname, '../packages/db/drizzle'));

const manifest = {
  initial_stage: 'onboarding',
  stages: {
    onboarding: {
      id: 'onboarding',
      description: 'Setup phase',
      flow: 'setup_flow',
      allowedTools: ['listOrders'],
      next: 'active',
    },
    active: {
      id: 'active',
      description: 'Full access',
    },
  },
};

const flows = [
  {
    id: 'setup_flow',
    state: 'onboarding',
    description: 'Setup',
    steps: [
      { id: 'orders', goal: 'Review orders', tool: 'listOrders', type: 'tool' },
    ],
  },
];

const sdk = {
  getFunctions: () => [],
  getStates: () => [],
  getPolicies: () => [],
  getFlows: () => flows,
  getPipelineManifest: () => manifest,
  getAttributes: () => [],
  invoke: async () => ({ ok: true, durationMs: 0 }),
};

const externalId = 'test-user';
const { customers } = await import('@aelio/db');
const now = new Date();
const customerId = randomUUID();
await database.db.insert(customers).values({
  id: customerId,
  externalId,
  displayName: externalId,
  createdAt: now,
  updatedAt: now,
  metadata: { lifecycleState: 'onboarding' },
});

await upsertPipelineState(database.db, customerId, 'onboarding');

const ctx = await evaluatePipeline({
  database,
  internalCustomerId: customerId,
  externalId,
  userMessage: 'show my orders',
  manifest,
  flows,
  attributes: [],
  authenticated: false,
});

if (!ctx.enabled || ctx.globalStage !== 'onboarding') {
  console.error('FAIL: expected onboarding stage, got', ctx);
  process.exit(1);
}

if (!ctx.activeFlow || ctx.currentStep?.tool !== 'listOrders') {
  console.error('FAIL: expected listOrders step, got', ctx.currentStep);
  process.exit(1);
}

console.log('PASS: pipeline phase 1 smoke test');
database.close();
