/** Mirrors SafetyLevel without importing index (avoids circular exports). */
type SafetyLevel = 'read' | 'write' | 'destructive';

/**
 * Named reusable access buckets for pipeline/lifecycle YAML and SDK config.
 * Expand into flat allow/block lists before enforcement — the wire protocol
 * still carries only tools / intents / safety arrays.
 */
export type ToolGroupDefinition = {
  tools?: string[];
  intents?: string[];
  safety?: SafetyLevel[];
};

export type ToolAccessSelectors = {
  allowedTools?: string[];
  blockedTools?: string[];
  allowedIntents?: string[];
  blockedIntents?: string[];
  allowedSafety?: SafetyLevel[];
  blockedSafety?: SafetyLevel[];
  allowedGroups?: string[];
  blockedGroups?: string[];
};

function unique(values: string[] | undefined): string[] | undefined {
  if (!values || values.length === 0) return undefined;
  return [...new Set(values)];
}

function uniqueSafety(values: SafetyLevel[] | undefined): SafetyLevel[] | undefined {
  if (!values || values.length === 0) return undefined;
  return [...new Set(values)];
}

function resolveGroup(
  groups: Record<string, ToolGroupDefinition>,
  groupId: string,
  usedIn: 'allowed_groups' | 'blocked_groups',
): ToolGroupDefinition {
  const group = groups[groupId];
  if (!group) {
    throw new Error(`Unknown tool group "${groupId}" referenced in ${usedIn}`);
  }
  return group;
}

function collectFromGroups(
  groups: Record<string, ToolGroupDefinition>,
  groupIds: string[] | undefined,
  usedIn: 'allowed_groups' | 'blocked_groups',
): { tools: string[]; intents: string[]; safety: SafetyLevel[] } {
  const tools: string[] = [];
  const intents: string[] = [];
  const safety: SafetyLevel[] = [];
  for (const groupId of groupIds ?? []) {
    const group = resolveGroup(groups, groupId, usedIn);
    if (group.tools) tools.push(...group.tools);
    if (group.intents) intents.push(...group.intents);
    if (group.safety) safety.push(...group.safety);
  }
  return { tools, intents, safety };
}

/**
 * Merge named groups with individual selectors. Groups contribute to allow or
 * block lists depending on whether they appear under allowedGroups or
 * blockedGroups. Explicit tool/intent/safety entries always union in.
 * Unknown group ids throw.
 */
export function expandToolAccess(
  groups: Record<string, ToolGroupDefinition> | undefined,
  access: ToolAccessSelectors,
): Omit<ToolAccessSelectors, 'allowedGroups' | 'blockedGroups'> {
  const catalog = groups ?? {};
  const allowed = collectFromGroups(catalog, access.allowedGroups, 'allowed_groups');
  const blocked = collectFromGroups(catalog, access.blockedGroups, 'blocked_groups');

  return {
    allowedTools: unique([...(allowed.tools), ...(access.allowedTools ?? [])]),
    blockedTools: unique([...(blocked.tools), ...(access.blockedTools ?? [])]),
    allowedIntents: unique([...(allowed.intents), ...(access.allowedIntents ?? [])]),
    blockedIntents: unique([...(blocked.intents), ...(access.blockedIntents ?? [])]),
    allowedSafety: uniqueSafety([...(allowed.safety), ...(access.allowedSafety ?? [])]),
    blockedSafety: uniqueSafety([...(blocked.safety), ...(access.blockedSafety ?? [])]),
  };
}
