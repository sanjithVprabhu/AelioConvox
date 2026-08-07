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

function stepByName(steps: Array<{ name: string; detail: string }>, name: string) {
  return steps.find((step) => step.name === name);
}

function stepsByPrefix(steps: Array<{ name: string; detail: string }>, prefix: string) {
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

/** One bordered pipeline block per user message. */
export function logTurnPipeline(details: {
  utterance: string;
  turnId: string;
  reply: string;
  tier?: string;
  llmCalls?: number;
  steps: Array<{ name: string; detail: string }>;
}): void {
  const { utterance, turnId, reply, tier, llmCalls, steps } = details;

  const conductor = stepByName(steps, 'Conductor.Select');
  const conductorHarness =
    conductor?.detail.match(/harness=([^\s—]+)/)?.[1]
    ?? conductor?.detail.match(/harness=(\w+)/)?.[1]
    ?? null;

  const proposePath = stepByName(steps, 'ProposePath');
  const proposed = parseProposedAbilities(proposePath?.detail);
  const proposedTools = proposed.filter(isToolAbility);

  const invokeCalls = steps.filter((step) => step.name === 'Invoke.Call');
  const toolsCalled = invokeCalls.map((step) => step.detail.replace(/\s+success$/i, ''));

  const orchestrate = stepsByPrefix(steps, 'Orchestrate.');
  const flowSteps = steps.filter((step) =>
    step.name.startsWith('Flow') || step.name === 'FlowMatch' || step.name === 'FlowGate',
  );

  const analyseParts: string[] = [];
  if (stepByName(steps, 'Sense')) analyseParts.push('sense');
  if (stepByName(steps, 'SplitClauses')) analyseParts.push('split clauses');
  const depth = stepByName(steps, 'ClassifyDepth') ?? stepByName(steps, 'Understand.Depth');
  if (depth) analyseParts.push(depth.detail);
  if (stepByName(steps, 'SituationKey')) analyseParts.push('situation key');
  if (tier) analyseParts.push(`tier=${tier}`);

  let checkingTools = 'tier lookup + ability registry';
  if (orchestrate.length) {
    checkingTools = orchestrate.map((step) => `${step.name}: ${step.detail}`).join(' · ');
  } else if (stepByName(steps, 'ProposePath.Retrieve')) {
    checkingTools = `retrieval shortlist · ${stepByName(steps, 'ProposePath.Retrieve')!.detail}`;
  } else if (conductorHarness) {
    checkingTools = `conductor → ${conductorHarness}`;
  }

  let foundTool: string;
  if (toolsCalled.length) {
    foundTool = toolsCalled.join(', ');
  } else if (proposedTools.length) {
    foundTool = `proposed [${proposedTools.join(', ')}] but not invoked`;
  } else if (proposed.length) {
    foundTool = `(none — path: ${proposed.join(' → ')})`;
  } else if (conductorHarness === 'quick_reply' || conductorHarness === 'understand_intent') {
    foundTool = `(skipped — conductor used ${conductorHarness}, no tool dispatch)`;
  } else if (conductorHarness === 'escalate') {
    foundTool = '(none — escalated but ProposePath did not select a tenant tool)';
  } else {
    foundTool = '(none)';
  }

  const flowDetail = flowSteps.length
    ? flowSteps.map((step) => `${step.name}: ${step.detail}`).join(' → ')
    : orchestrate.find((step) => step.name === 'Orchestrate.Graph')?.detail
      ?? '(single-step — no flow graph)';

  const outputDetail = toolsCalled.length
    ? `${toolsCalled.length} tool result(s) in evidence`
    : proposedTools.length
      ? 'waiting on tool — execution did not complete'
      : 'no tool data (LLM answered from prompt/context only)';

  const llmDetail = (llmCalls ?? 0) > 0
    ? `yes — ${llmCalls} LLM call(s) for propose and/or synthesize`
    : 'no — template or deterministic reply';

  const preview = reply.length > 140 ? `${reply.slice(0, 140)}…` : reply;

  const lines = [
    `message: "${utterance.slice(0, 100)}${utterance.length > 100 ? '…' : ''}"`,
    `turn_id: ${turnId}`,
    '',
    '1. Harness Called',
    `2. Analysing — ${analyseParts.join(' · ') || 'processing user message'}`,
    `3. Checking Tools — ${checkingTools}`,
    `4. Found Tool: ${foundTool}`,
    `5. Checking Flow — ${flowDetail}`,
    `6. Output — ${outputDetail}`,
    `7. Sending output as input to LLM — ${llmDetail}`,
    `8. Final Response — "${preview}"`,
    '9. Return',
  ];

  writePipeline(lines.join('\n'));
}
