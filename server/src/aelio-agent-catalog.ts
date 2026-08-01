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
  const capabilities = tools
    .map((tool) => tool.capability_tags[0])
    .filter((capability): capability is string => Boolean(capability));
  const states = toAgentStates(registration.states ?? [], tools, capabilities);
  const policies = toAgentPolicies(
    registration.policies ?? [],
    tools,
    options.memoryEnabled === true,
  );
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
    // Closed Mother-DSL artifacts are registered separately and are never
    // weakened into the older goal/step representation.
    flows: [],
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
    id: fn.name,
    name: fn.name,
    version: '1',
    capability_tags: [fn.intent ?? snakeCase(fn.name)],
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
    tools.map((tool) => [tool.id, tool.capability_tags[0] ?? tool.id]),
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

function snakeCase(value: string): string {
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
