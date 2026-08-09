const BORDER = '---';

const INTERNAL_LLM_PREFIX = '/internal/aelio/llm';

/** Drop noisy per-request pino lines for LLM modem routes (complete/embed). */
export function createQuietLoggerStream(): { write(chunk: string): void } {
  return {
    write(chunk: string) {
      if (shouldSuppressPinoLine(chunk)) return;
      process.stdout.write(chunk);
    },
  };
}

function shouldSuppressPinoLine(chunk: string): boolean {
  if (!chunk.includes(INTERNAL_LLM_PREFIX)) return false;
  return chunk.includes('"incoming request"') || chunk.includes('"request completed"');
}

function writePipeline(block: string): void {
  process.stdout.write(`${BORDER}\n${block}\n${BORDER}\n`);
}

type TraceStep = { name: string; detail: string };

function stepByName(steps: TraceStep[], name: string) {
  return steps.find((step) => step.name === name);
}

function stepsByPrefix(steps: TraceStep[], prefix: string) {
  return steps.filter((step) => step.name.startsWith(prefix));
}

function parseProposedAbilities(detail: string | undefined): string[] {
  if (!detail) return [];
  const bracket = detail.match(/\[([^\]]*)\]/);
  if (!bracket?.[1]?.trim()) return [];
  return bracket[1].split(',').map((part) => part.trim()).filter(Boolean);
}

function isToolAbility(id: string): boolean {
  return !/^(Express|State|Sense|Learn|Understand|Judge|Bind|Registry|Invoke)\./i.test(id)
    && !id.startsWith('std.');
}

function clip(text: string, max: number): string {
  const t = text.trim();
  return t.length > max ? `${t.slice(0, max)}…` : t;
}

function harnessFromSelect(detail: string | undefined): string | null {
  if (!detail) return null;
  return detail.match(/harness=([^\s—]+)/)?.[1]
    ?? detail.match(/harness=(\S+)/)?.[1]
    ?? null;
}

function parseMode(detail: string | undefined): { mode: string; legacy: boolean; reason: string } {
  if (!detail) return { mode: 'unknown', legacy: false, reason: 'no Harness.Mode step' };
  const mode = detail.match(/mode=(\w+)/)?.[1] ?? 'unknown';
  const legacy = /legacy_spine=true/.test(detail);
  const reason = detail.match(/reason=(.+)$/)?.[1]?.trim() ?? detail;
  return { mode, legacy, reason };
}

/**
 * One bordered pipeline block per user message.
 * Reads the Rust turn-trace steps and prints an honest disposition — never
 * "Found Tool: skipped" when no tool was ever selected.
 */
export function logTurnPipeline(details: {
  utterance: string;
  turnId: string;
  reply: string;
  tier?: string;
  llmCalls?: number;
  steps: TraceStep[];
  suspended?: boolean;
  openedLoop?: boolean;
}): void {
  const { utterance, turnId, reply, tier, llmCalls, steps } = details;

  const modeStep = stepByName(steps, 'Harness.Mode');
  const { mode, legacy, reason: modeReason } = parseMode(modeStep?.detail);
  const authority = mode === 'agent_loop'
    ? 'AGENT LOOP'
    : legacy
      ? 'LEGACY agent spine'
      : 'SOL Path B';
  const agentLoop = stepByName(steps, 'AgentLoop.Run');
  const agentLoopToolCalls = Number(agentLoop?.detail.match(/tool_calls=(\d+)/)?.[1] ?? 0);

  const cutover = stepByName(steps, 'Conductor.Cutover');
  const select = stepByName(steps, 'Conductor.Select');
  const failClosed = stepByName(steps, 'Conductor.FailClosed');
  const load = stepByName(steps, 'Harness.Load');
  const harnessTool = stepByName(steps, 'Harness.Tool');
  const park = stepByName(steps, 'Harness.Park');
  const stack = stepByName(steps, 'Harness.Stack');
  const shadow = stepByName(steps, 'Conductor.Shadow');

  const harnessId =
    harnessFromSelect(select?.detail)
    ?? (cutover?.detail.match(/route=([^\s]+)/)?.[1] ?? null)
    ?? (failClosed ? 'fail_closed' : null);

  const proposePath = stepByName(steps, 'ProposePath');
  const proposed = parseProposedAbilities(proposePath?.detail);
  const proposedTools = proposed.filter(isToolAbility);

  const invokeCalls = steps.filter((step) => step.name === 'Invoke.Call');
  const toolsCalled = invokeCalls.map((step) => step.detail.replace(/\s+success$/i, ''));

  const solToolId = harnessId?.startsWith('tool.')
    ? harnessId
    : harnessTool?.detail.match(/(?:id|program id)=([^\s]+)/)?.[1]
      ?? harnessTool?.detail.match(/executed Sol program id=([^\s]+)/)?.[1]
      ?? null;

  // ── Route / harness ──────────────────────────────────────────────────────
  let routeLine: string;
  if (agentLoop) {
    routeLine = 'provider-native model → tool → result loop';
  } else if (failClosed) {
    routeLine = `fail-closed — ${failClosed.detail}`;
  } else if (cutover && harnessId) {
    routeLine = `${harnessId} (conductor.root cutover)`;
  } else if (harnessId && select?.detail.includes('resume')) {
    routeLine = `${harnessId} (resume)`;
  } else if (harnessId && (solToolId || harnessTool)) {
    routeLine = `${harnessId} (installed Sol tool harness)`;
  } else if (harnessId) {
    routeLine = `${harnessId}${select?.detail.includes('cutover') ? ' (cutover)' : ''}`;
  } else if (stack) {
    routeLine = `stack — ${stack.detail}`;
  } else {
    routeLine = 'unresolved (no Conductor.Select)';
  }

  // ── Tool disposition (honest; never "found then skipped") ────────────────
  let toolLine: string;
  if (agentLoopToolCalls > 0) {
    toolLine = `${agentLoopToolCalls} kernel-gated call(s)`;
  } else if (agentLoop) {
    toolLine = 'none';
  } else if (toolsCalled.length) {
    toolLine = `invoked SDK/ability tools: ${toolsCalled.join(', ')}`;
  } else if (harnessTool || solToolId) {
    toolLine = `ran Sol harness ${solToolId ?? harnessTool!.detail}`;
  } else if (proposedTools.length) {
    toolLine = `proposed [${proposedTools.join(', ')}] but did not invoke`;
  } else if (proposed.length) {
    toolLine = `no tenant tool — ability path: ${proposed.join(' → ')}`;
  } else if (harnessId === 'quick_reply' || harnessId === 'understand_intent' || harnessId === 'wait_for_user') {
    toolLine = `none — route is ${harnessId} (by design, not a failed lookup)`;
  } else if (harnessId === 'escalate') {
    toolLine = 'none — escalated; ProposePath did not pick a tenant tool';
  } else if (harnessId?.startsWith('tool.')) {
    toolLine = `selected ${harnessId} (see harness steps)`;
  } else {
    toolLine = 'none';
  }

  // ── Flow / park ──────────────────────────────────────────────────────────
  const flowSteps = steps.filter((step) =>
    step.name.startsWith('Flow') || step.name === 'FlowMatch' || step.name === 'FlowGate',
  );
  const orchestrate = stepsByPrefix(steps, 'Orchestrate.');
  let flowLine: string;
  if (park) {
    flowLine = `parked — ${park.detail}`;
  } else if (details.suspended) {
    flowLine = 'suspended (waiting on user / external)';
  } else if (flowSteps.length) {
    flowLine = flowSteps.map((s) => `${s.name}: ${s.detail}`).join(' → ');
  } else if (orchestrate.find((s) => s.name === 'Orchestrate.Graph')) {
    flowLine = orchestrate.find((s) => s.name === 'Orchestrate.Graph')!.detail;
  } else {
    flowLine = 'none (single-shot turn)';
  }

  // ── Evidence / LLM ───────────────────────────────────────────────────────
  let evidenceLine: string;
  if (agentLoopToolCalls > 0) {
    evidenceLine = 'structured tool result(s) processed by agent loop';
  } else if (toolsCalled.length || harnessTool || solToolId) {
    evidenceLine = 'tool/harness result used for reply';
  } else if (proposedTools.length) {
    evidenceLine = 'tool proposed but not executed';
  } else if ((llmCalls ?? 0) > 0) {
    evidenceLine = 'LLM synthesis / propose (no tool evidence)';
  } else {
    evidenceLine = 'template or deterministic reply';
  }

  const llmLine = (llmCalls ?? 0) > 0
    ? `${llmCalls} call(s)`
    : '0 (no model call this turn)';

  const analyseBits: string[] = [];
  if (stepByName(steps, 'Sense')) analyseBits.push('sense');
  if (stepByName(steps, 'SplitClauses')) analyseBits.push('split clauses');
  const depth = stepByName(steps, 'ClassifyDepth') ?? stepByName(steps, 'Understand.Depth');
  if (depth) analyseBits.push(depth.detail);
  if (tier) analyseBits.push(`tier=${tier}`);
  if (load) analyseBits.push(`load ${load.detail}`);

  const lines = [
    `message:  "${clip(utterance, 100)}"`,
    `turn_id:  ${turnId}`,
    `mode:     ${mode} · ${authority}`,
    `reason:   ${modeReason}`,
    '',
    `1. Route        ${routeLine}`,
    `2. Analyse      ${analyseBits.join(' · ') || '—'}`,
    `3. Tool         ${toolLine}`,
    `4. Flow         ${flowLine}`,
    `5. Evidence     ${evidenceLine}`,
    `6. LLM          ${llmLine}`,
    shadow ? `7. Shadow       ${shadow.detail}` : '7. Shadow       —',
    `8. Reply        "${clip(reply, 160)}"`,
    details.suspended || details.openedLoop
      ? `9. State        suspended=${Boolean(details.suspended)} opened_loop=${Boolean(details.openedLoop)}`
      : '9. Done',
  ];

  writePipeline(lines.join('\n'));
}
