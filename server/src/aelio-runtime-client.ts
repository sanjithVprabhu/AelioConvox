import { z } from 'zod';

const ErrorBodySchema = z.union([
  z.object({
    error: z.object({
      code: z.string(),
      detail: z.string(),
    }),
  }).transform(({ error }) => error),
  z.object({
    code: z.string(),
    message: z.string(),
  }).transform(({ code, message }) => ({ code, detail: message })),
]);

const TurnReplySchema = z.discriminatedUnion('outcome', [
  z.object({
    outcome: z.literal('completed'),
    bag: z.unknown(),
    bag_hash: z.string(),
  }),
  z.object({
    outcome: z.literal('parked'),
    park_nid: z.string(),
    event_key: z.string().nullable(),
    bag: z.unknown(),
    bag_hash: z.string(),
  }),
]);

const AgentTurnReplySchema = z.object({
  reply: z
    .object({
      text: z.string(),
      frame: z
        .object({
          frame: z.literal('render'),
          frame_id: z.string(),
          mode: z.enum(['append', 'patch', 'replace_turn']),
          turn_id: z.string(),
          blocks: z.array(z.record(z.unknown())).min(1),
        })
        .passthrough()
        .optional(),
    })
    .passthrough(),
  suspended: z.boolean(),
  llm_calls: z.number().int().nonnegative(),
  steps: z.array(z.object({ name: z.string(), detail: z.string() })),
}).passthrough();

const AgentScheduledJobSchema = z.object({
  id: z.string().min(1),
  kind: z.string().min(1),
  payload: z.unknown(),
  state: z.enum(['scheduled', 'leased', 'retry_waiting', 'terminal']),
  scheduled_at_ms: z.number().int(),
  owner: z.string().nullable(),
  lease_expires_at_ms: z.number().int().nullable(),
  attempts: z.number().int().nonnegative(),
  max_attempts: z.number().int().positive(),
  last_error: z.string().nullable(),
  terminal: z.unknown().nullable(),
});

export type AgentScheduledJob = z.infer<typeof AgentScheduledJobSchema>;

const ProactiveDecisionSchema = z.union([
  z.object({ enqueued: z.object({ job_id: z.string().min(1) }) }),
  z.object({ already_enqueued: z.object({ job_id: z.string().min(1) }) }),
  z.object({ suppressed: z.object({ reason: z.string().min(1) }) }),
]);
export type ProactiveDecision = z.infer<typeof ProactiveDecisionSchema>;

export type AelioFlowPush = {
  tenant: string;
  flow_id: string;
  flow_rev: string;
  program: unknown;
  prompts?: Array<{
    target_id: string;
    template_id: string;
    version: string;
    body: string;
    slots?: Array<{
      name: string;
      ty: 'null' | 'bool' | 'int' | 'float' | 'str' | 'list' | 'map';
      sensitivity: 'public' | 'internal' | 'pii' | 'secret';
    }>;
    layers?: Array<{ id: string; text: string }>;
  }>;
  targets: Array<{
    id: string;
    class: 'compute' | 'io' | 'model' | 'tool' | 'flow';
    effect: 'pure' | 'read' | 'write' | 'external';
    input_imprint: string;
    output_imprint: string;
    bounded:
      | { kind: 'cost'; max_units: number }
      | { kind: 'deadline'; max_ms: number }
      | { kind: 'registered_flow' };
    policy_tags?: string[];
    origin?: 'tenant' | 'vendor';
  }>;
};

export type AelioTurnSubmit = {
  tenant: string;
  instance_id: string;
  flow_id: string;
  flow_rev: string;
  input: unknown;
};

export type AelioTurnReply = z.infer<typeof TurnReplySchema>;

export class AelioRuntimeClient {
  private readonly baseUrl: string;

  constructor(
    url: string,
    private readonly token: string,
    private readonly timeoutMs = 35_000,
  ) {
    this.baseUrl = url.replace(/\/+$/, '');
    if (!this.baseUrl || !token) {
      throw new Error('Aelio Rust runtime URL and token are required');
    }
  }

  async ready(): Promise<void> {
    await this.request('/readyz', { method: 'GET' }, z.object({ status: z.literal('ready') }));
  }

  async pushFlow(flow: AelioFlowPush): Promise<void> {
    await this.request(
      '/v1/flows',
      { method: 'POST', body: JSON.stringify(flow) },
      z.object({ accepted: z.literal(true) }),
    );
  }

  async installBaselineConversation(
    tenant: string,
  ): Promise<{ flow_id: string; flow_rev: string }> {
    return this.request(
      '/v1/system/flows/conversation',
      { method: 'POST', body: JSON.stringify({ tenant }) },
      z.object({
        accepted: z.literal(true),
        flow_id: z.string().min(1),
        flow_rev: z.string().min(1),
      }).transform(({ flow_id, flow_rev }) => ({ flow_id, flow_rev })),
    );
  }

  submitTurn(turn: AelioTurnSubmit): Promise<AelioTurnReply> {
    return this.request(
      '/v1/turns',
      { method: 'POST', body: JSON.stringify(turn) },
      TurnReplySchema,
    );
  }

  async pushAgentCatalog(catalog: unknown): Promise<void> {
    await this.request(
      '/agent/v1/catalog',
      { method: 'POST', body: JSON.stringify(catalog) },
      z.object({ status: z.literal('active') }).passthrough(),
    );
  }

  submitAgentTurn(turn: {
    turn_id: string;
    user_id: string;
    utterance: string;
    channel: string;
  }): Promise<z.infer<typeof AgentTurnReplySchema>> {
    return this.request(
      '/agent/v1/turns',
      { method: 'POST', body: JSON.stringify(turn) },
      AgentTurnReplySchema,
    );
  }

  async setAgentUserState(command: {
    command_id: string;
    user_id: string;
    state_id: string;
    reason?: string;
  }): Promise<void> {
    await this.request(
      '/agent/v1/users/state',
      { method: 'POST', body: JSON.stringify(command) },
      z.object({ applied: z.literal(true) }),
    );
  }

  evaluateProactive(input: {
    candidate: {
      user_id: string;
      fingerprint: string;
      payload: unknown;
      opted_in: boolean;
      deterministic_gate: boolean;
      confidence_millis: number;
      min_confidence_millis: number;
    };
    policy: {
      min_cadence_ms: number;
      max_enqueues_per_day: number;
      suppression_ms: number;
      max_job_attempts: number;
    };
  }): Promise<ProactiveDecision> {
    return this.request(
      '/agent/v1/proactive/evaluate',
      { method: 'POST', body: JSON.stringify(input) },
      ProactiveDecisionSchema,
    );
  }

  async leaseAgentJobs(owner: string, kind: string, limit = 8): Promise<AgentScheduledJob[]> {
    const result = await this.request(
      '/agent/v1/admin/workers/tick',
      {
        method: 'POST',
        body: JSON.stringify({ owner, kind, lease_ms: 30_000, limit }),
      },
      z.object({ leased_jobs: z.array(AgentScheduledJobSchema), reconciled: z.array(z.unknown()) }),
    );
    return result.leased_jobs;
  }

  async completeAgentJob(owner: string, jobId: string): Promise<void> {
    await this.request(
      '/agent/v1/admin/workers/complete',
      { method: 'POST', body: JSON.stringify({ owner, job_id: jobId }) },
      z.object({ completed: z.literal(true) }),
    );
  }

  async failAgentJob(
    owner: string,
    jobId: string,
    reason: string,
    retryInMs = 5_000,
  ): Promise<void> {
    await this.request(
      '/agent/v1/admin/workers/fail',
      {
        method: 'POST',
        body: JSON.stringify({ owner, job_id: jobId, reason, retry_in_ms: retryInMs }),
      },
      z.object({ failed: z.literal(true), state: z.string() }),
    );
  }

  private async request<T>(
    path: string,
    init: RequestInit,
    schema: z.ZodType<T>,
  ): Promise<T> {
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), this.timeoutMs);
    try {
      const response = await fetch(`${this.baseUrl}${path}`, {
        ...init,
        headers: {
          authorization: `Bearer ${this.token}`,
          'content-type': 'application/json',
          ...init.headers,
        },
        signal: controller.signal,
      });
      const body: unknown = await response.json().catch(() => null);
      if (!response.ok) {
        const parsed = ErrorBodySchema.safeParse(body);
        const detail = parsed.success
          ? `${parsed.data.code}: ${parsed.data.detail}`
          : `HTTP ${response.status}`;
        throw new Error(`Aelio Rust runtime rejected request: ${detail}`);
      }
      const parsed = schema.safeParse(body);
      if (!parsed.success) {
        throw new Error(`Aelio Rust runtime returned an invalid response: ${parsed.error.message}`);
      }
      return parsed.data;
    } finally {
      clearTimeout(timeout);
    }
  }
}
