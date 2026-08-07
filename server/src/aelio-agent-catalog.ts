import type {
  FunctionDefinition,
  PolicyDefinition,
  RegisterMessage,
  StateDefinition,
} from '@aelio/protocol';

type Json = Record<string, unknown>;

/** Mechanical SDK-schema → Rust agent catalog translation; contains no runtime decisions. */
export function buildAgentCatalog(
  tenant: string,
  registration: RegisterMessage,
  options: { memoryEnabled?: boolean } = {},
): Json {
  const tools = registration.functions.map(toAgentTool);
  const toolIds = new Set<string>();
  for (const tool of tools) {
    if (toolIds.has(tool.id)) {
      throw new Error(
        `SDK function names collide after canonicalization at "${tool.id}"; rename one function`,
      );
    }
    toolIds.add(tool.id);
  }
  const capabilities = tools
    .map((tool) => tool.capability_tags[0])
    .filter((capability): capability is string => Boolean(capability));
  const states = toAgentStates(registration.states ?? [], tools, capabilities);
  const policies = toAgentPolicies(
    registration.policies ?? [],
    tools,
    options.memoryEnabled === true,
  );
  const capabilityByTool = new Map(
    tools.flatMap((tool) => [
      [tool.id, tool.capability_tags[0] ?? tool.id],
      [tool.name, tool.capability_tags[0] ?? tool.id],
    ] as const),
  );
  const flows = (registration.flows ?? []).map((flow) => flow.aelio
    ? { id: flow.id, name: flow.description, ...flow.aelio }
    : {
        id: flow.id,
        version: '1',
        name: flow.description,
        activation: {
          hard_preconditions: [{ op: 'eq', path: 'state', value: flow.state }],
          trigger_surface: [flow.id, flow.description],
          margin_threshold: 0.7,
        },
        learnable: false,
        preemption: 'hold',
        steps: flow.steps.map((step) => ({
          id: step.id,
          intent: step.goal,
          postcondition: { op: 'true' },
          admissible: step.tool ? [capabilityByTool.get(step.tool) ?? step.tool] : [],
          on_violation: 'escape',
          suspendable: false,
        })),
        escape: { kind: 'free_range' },
        terminal_states: [],
        ttl_secs: 300,
        max_attempts: Math.max(1, flow.steps.length),
        lowering: null,
      });
  return {
    tenant_id: tenant,
    mode: 'live',
    tools,
    personalities: [{
      id: 'sdk',
      voice: {
        register: 'friendly',
        verbosity: 'medium',
        formality: 'casual',
        emoji_policy: 'sparse',
      },
      lexicon: { preferred: [], forbidden: [] },
      constraints: [
        registration.persona?.trim() || 'Be concise, accurate, and helpful.',
        registration.productBrief?.trim() || 'Use only registered product capabilities.',
        ...(registration.policies ?? []).map(
          (policy) =>
            `${policy.severity === 'hard' ? 'Required product rule' : 'Product guidance'} ` +
            `[${policy.id}]: ${policy.description}`,
        ),
      ],
      templates: {},
    }],
    states,
    policies,
    // Semantic WHAT plus symbolic HOW are admitted together. Rust resolves every `$cap:` call to
    // one exact tool proxy, gates the immutable artifact, and inserts the resulting pin before this
    // catalog snapshot can become active.
    flows,
    flow_artifacts: {},
    attributes: [],
  };
}

function toAgentTool(fn: FunctionDefinition) {
  const params = Object.entries(fn.params).map(([name, raw]) => {
    const spec = typeof raw === 'string' ? { type: raw } : asObject(raw);
    const rawType = String(spec.type ?? 'string').replace(/\?$/, '');
    const optional = String(spec.type ?? '').endsWith('?') || spec.optional === true;
    const constraint =
      Array.isArray(spec.enum)
        ? { range: undefined, kind: 'enum', allowed: spec.enum.map(String) }
        : typeof spec.format === 'string'
          ? { kind: 'format', format: spec.format }
          : undefined;
    return {
      name,
      type_name: agentType(rawType),
      required: !optional,
      constraint: constraint
        ? constraint.kind === 'enum'
          ? { enum: { allowed: constraint.allowed } }
          : { format: { kind: constraint.format } }
        : null,
      source: { kind: 'user' },
      repair: null,
      prompt_hint: typeof spec.description === 'string' ? spec.description : null,
      sensitivity: 'none',
      default: null,
      depends_on: [],
    };
  });
  const outputFields = Object.fromEntries(
    Object.entries(fn.output ?? {}).map(([name, field]) => [
      name,
      {
        path: field.path ?? `$.${name}`,
        type_name: agentType(field.type),
        sensitivity: field.sensitivity,
        meaning: field.meaning,
      },
    ]),
  );
  return {
    // The Mother DSL deliberately has a narrower identifier alphabet than JavaScript. Keep the
    // public SDK name in `name`, while `id` is the stable kernel/artifact identifier. Invocation
    // translates back to `name` at the SDK boundary.
    id: canonicalAgentToolId(fn.name),
    name: fn.name,
    version: '1',
    capability_tags: [fn.intent ?? canonicalAgentToolId(fn.name)],
    // The SDK schema does not declare result-set coverage. Preserve that fact explicitly:
    // Unknown is admissible for execution but blocks promotion of aggregate workflows.
    contract: {
      effect_class: fn.safety === 'read' ? 'read' : 'write',
      completeness: { kind: 'unknown' },
      returns_entity: fn.outputRole ?? `sdk:${canonicalAgentToolId(fn.name)}:result`,
      pushdown: [],
      max_result_rows: null,
      row_scoped: false,
    },
    effect: fn.safety === 'read' ? 'read' : 'write',
    effectful: fn.safety !== 'read',
    idempotent: fn.safety === 'read',
    dry_run_available: false,
    params,
    output_semantics: { fields: outputFields, role_hint: fn.outputRole ?? null },
    continuations: [],
    errors: [],
  };
}

function toAgentStates(
  states: StateDefinition[],
  tools: ReturnType<typeof toAgentTool>[],
  allCapabilities: string[],
) {
  const capabilityByTool = new Map(
    tools.flatMap((tool) => {
      const capability = tool.capability_tags[0] ?? tool.id;
      return [[tool.id, capability], [tool.name, capability]] as const;
    }),
  );
  const capability = (name: string) => capabilityByTool.get(name) ?? name;
  const mapped = states.map((state) => ({
    id: state.id,
    name: state.description,
    permission_envelope: (
      state.allowedTools?.length
        ? state.allowedTools.map(capability)
        : allCapabilities
    ).filter((item) => !(state.blockedTools ?? []).map(capability).includes(item)),
    direction: null,
    entry_conditions: (state.guards?.requires_fields ?? []).map((field) => ({
      op: 'present',
      path: `slot.${field}`,
    })),
    exit_edges: [],
    timeout: null,
  }));
  if (!mapped.some((state) => state.id === 'unauthenticated')) {
    mapped.unshift({
      id: 'unauthenticated',
      name: 'Default',
      permission_envelope: allCapabilities,
      direction: null,
      entry_conditions: [],
      exit_edges: [],
      timeout: null,
    });
  }
  return mapped;
}

function toAgentPolicies(
  policies: PolicyDefinition[],
  tools: ReturnType<typeof toAgentTool>[],
  memoryEnabled: boolean,
) {
  const declaredPolicies = policies
    .filter((policy) => policy.aelio)
    .map((policy) => ({
      id: policy.id,
      ...policy.aelio!,
    }));
  // Registering an effectful SDK tool is the tenant's explicit grant for that exact tool, not a
  // blanket mutation grant. Rust still requires confirmation and any higher-priority deny wins.
  const toolPolicies = tools
    .filter((tool) => tool.effectful)
    .map((tool) => ({
      id: `sdk.registered-tool.${tool.id}`,
      effect: 'allow',
      subject: {},
      action: { tool_id: tool.id },
      condition: { op: 'true' },
      reason_code:
        policies.map((policy) => policy.id).join(',') || 'registered_effectful_sdk_tool',
      priority: 1,
    }));
  const memoryPolicies = memoryEnabled
    ? ['memory.write', 'memory.delete'].map((capability) => ({
        id: `tenant.feature.${capability}`,
        effect: 'allow',
        subject: {},
        action: { capability },
        condition: { op: 'true' },
        reason_code: 'tenant_memory_feature_enabled',
        priority: 1,
      }))
    : [];
  return [...declaredPolicies, ...toolPolicies, ...memoryPolicies];
}

function agentType(type: string): string {
  return ({
    auto: 'auto',
    string: 'str',
    // JSON/JavaScript has one numeric type; accepting either Rust numeric
    // representation is safer than rejecting integral JSON as a non-float.
    number: 'auto',
    integer: 'int',
    boolean: 'bool',
    array: 'list',
    object: 'map',
  } as Record<string, string>)[type] ?? 'str';
}

export function canonicalAgentToolId(value: string): string {
  return value
    .replace(/([a-z0-9])([A-Z])/g, '$1_$2')
    .replace(/[^A-Za-z0-9]+/g, '_')
    .replace(/^_+|_+$/g, '')
    .toLowerCase();
}

function asObject(value: unknown): Record<string, unknown> {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? value as Record<string, unknown>
    : {};
}
