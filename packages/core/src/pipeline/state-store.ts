import type { AelioDatabase } from '@aelio/db';
import {
  customerAttributes,
  customerFeatureState,
  customerPipelineState,
  customers,
} from '@aelio/db';
import type { EntryMethod, FeatureStage } from '@aelio/protocol';
import { and, eq } from 'drizzle-orm';
import { randomUUID } from 'node:crypto';

export type PipelineStateRecord = {
  globalStage: string;
  enteredAt: Date;
  entryMethod: EntryMethod;
};

export type FeatureStateRecord = {
  featureId: string;
  stage: FeatureStage;
  enteredAt: Date;
  entryMethod: EntryMethod;
};

export type CustomerAttributeRecord = {
  attributeId: string;
  value: unknown;
  source: string;
  verified: boolean;
  collectedAt: Date;
};

export async function getPipelineState(
  db: AelioDatabase['db'],
  internalCustomerId: string,
): Promise<PipelineStateRecord | null> {
  const rows = await db
    .select()
    .from(customerPipelineState)
    .where(eq(customerPipelineState.customerId, internalCustomerId))
    .limit(1);

  const row = rows[0];
  if (!row) {
    return null;
  }

  return {
    globalStage: row.globalStage,
    enteredAt: row.enteredAt,
    entryMethod: row.entryMethod as EntryMethod,
  };
}

export async function upsertPipelineState(
  db: AelioDatabase['db'],
  internalCustomerId: string,
  globalStage: string,
  entryMethod: EntryMethod = 'system_triggered',
): Promise<void> {
  const now = new Date();
  await db
    .insert(customerPipelineState)
    .values({
      customerId: internalCustomerId,
      globalStage,
      enteredAt: now,
      entryMethod,
    })
    .onConflictDoUpdate({
      target: customerPipelineState.customerId,
      set: {
        globalStage,
        enteredAt: now,
        entryMethod,
      },
    });

  const customerRows = await db
    .select({ externalId: customers.externalId })
    .from(customers)
    .where(eq(customers.id, internalCustomerId))
    .limit(1);

  const externalId = customerRows[0]?.externalId;
  if (externalId) {
    const { upsertCustomerLifecycleState } = await import('../lifecycle/index.js');
    await upsertCustomerLifecycleState(db, externalId, globalStage, 'pipeline: stage sync');
  }
}

export async function getFeatureStates(
  db: AelioDatabase['db'],
  internalCustomerId: string,
): Promise<FeatureStateRecord[]> {
  const rows = await db
    .select()
    .from(customerFeatureState)
    .where(eq(customerFeatureState.customerId, internalCustomerId));

  return rows.map((row) => ({
    featureId: row.featureId,
    stage: row.stage as FeatureStage,
    enteredAt: row.enteredAt,
    entryMethod: row.entryMethod as EntryMethod,
  }));
}

export async function upsertFeatureState(
  db: AelioDatabase['db'],
  internalCustomerId: string,
  featureId: string,
  stage: FeatureStage,
  entryMethod: EntryMethod = 'system_triggered',
): Promise<void> {
  const now = new Date();
  const existing = await db
    .select({ id: customerFeatureState.id })
    .from(customerFeatureState)
    .where(
      and(
        eq(customerFeatureState.customerId, internalCustomerId),
        eq(customerFeatureState.featureId, featureId),
      ),
    )
    .limit(1);

  if (existing[0]) {
    await db
      .update(customerFeatureState)
      .set({ stage, enteredAt: now, entryMethod })
      .where(eq(customerFeatureState.id, existing[0].id));
    return;
  }

  await db.insert(customerFeatureState).values({
    id: randomUUID(),
    customerId: internalCustomerId,
    featureId,
    stage,
    enteredAt: now,
    entryMethod,
  });
}

export async function getCustomerAttributes(
  db: AelioDatabase['db'],
  internalCustomerId: string,
): Promise<CustomerAttributeRecord[]> {
  const rows = await db
    .select()
    .from(customerAttributes)
    .where(eq(customerAttributes.customerId, internalCustomerId));

  return rows.map((row) => ({
    attributeId: row.attributeId,
    value: row.value,
    source: row.source,
    verified: row.verified ?? false,
    collectedAt: row.collectedAt,
  }));
}

export async function getAttributeValue(
  db: AelioDatabase['db'],
  internalCustomerId: string,
  attributeId: string,
): Promise<unknown | null> {
  const rows = await db
    .select()
    .from(customerAttributes)
    .where(eq(customerAttributes.customerId, internalCustomerId));

  const row = rows.find((entry) => entry.attributeId === attributeId);
  return row?.value ?? null;
}

export async function upsertCustomerAttribute(
  db: AelioDatabase['db'],
  internalCustomerId: string,
  attributeId: string,
  value: unknown,
  source: string = 'explicit_ask',
  verified: boolean = false,
): Promise<void> {
  const now = new Date();
  const existing = await db
    .select()
    .from(customerAttributes)
    .where(eq(customerAttributes.customerId, internalCustomerId));

  const match = existing.find((row) => row.attributeId === attributeId);

  if (match) {
    await db
      .update(customerAttributes)
      .set({ value, source, verified, collectedAt: now })
      .where(eq(customerAttributes.id, match.id));
  } else {
    await db.insert(customerAttributes).values({
      id: randomUUID(),
      customerId: internalCustomerId,
      attributeId,
      value,
      source,
      verified,
      collectedAt: now,
    });
  }

  const customerRows = await db
    .select({ metadata: customers.metadata })
    .from(customers)
    .where(eq(customers.id, internalCustomerId))
    .limit(1);

  const metadata = { ...(customerRows[0]?.metadata ?? {}), [attributeId]: value };
  await db.update(customers).set({ metadata, updatedAt: now }).where(eq(customers.id, internalCustomerId));
}

export async function hasAttribute(
  db: AelioDatabase['db'],
  internalCustomerId: string,
  attributeId: string,
): Promise<boolean> {
  const value = await getAttributeValue(db, internalCustomerId, attributeId);
  if (value === null || value === undefined || value === '' || value === false) {
    return false;
  }
  return true;
}
