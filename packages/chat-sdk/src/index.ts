/**
 * @aelio/chat-sdk — Aelio Render Protocol client (AELIO_RENDER_PROTOCOL.md)
 *
 * Closed kinds only. No HTML/JS crosses the wire. Validators mirror aelio-render.
 */

import { z } from 'zod';

export const PROTOCOL_ID = 'aelio-render@v1' as const;
export const CORE_KINDS = [
  'text@1',
  'code@1',
  'table@1',
  'chart@1',
  'diagram@1',
  'metric@1',
  'form@1',
  'choice@1',
  'confirm@1',
  'status@1',
  'media@1',
  'group@1',
] as const;

export type CoreKind = (typeof CORE_KINDS)[number];

const BlockMetaSchema = z
  .object({
    width: z.enum(['auto', 'full']).optional(),
    collapsed: z.boolean().optional(),
  })
  .strict();

export type Block = {
  block_id: string;
  kind: string;
  body: Record<string, unknown>;
  fallback?: Block;
  on?: Record<string, string>;
  meta?: z.infer<typeof BlockMetaSchema>;
};

const BlockSchema: z.ZodType<Block> = z.lazy(() =>
  z
    .object({
      block_id: z.string().min(1),
      kind: z.string().regex(/^.+@.+$/),
      body: z.record(z.unknown()),
      fallback: BlockSchema.optional(),
      on: z.record(z.string()).optional(),
      meta: BlockMetaSchema.optional(),
    })
    .strict(),
);

export const RenderFrameSchema = z
  .object({
    frame: z.literal('render'),
    frame_id: z.string().min(1),
    mode: z.enum(['append', 'patch', 'replace_turn']),
    turn_id: z.string().min(1),
    blocks: z.array(BlockSchema).min(1).max(64),
  })
  .strict();

export type RenderFrame = z.infer<typeof RenderFrameSchema>;

export const EventFrameSchema = z
  .object({
    frame: z.literal('event'),
    frame_id: z.string().min(1),
    turn_id: z.string().min(1),
    event: z
      .object({
        type: z.enum([
          'message',
          'copied',
          'row_selected',
          'point_selected',
          'node_selected',
          'submitted',
          'chosen',
          'confirmed',
          'render_degraded',
          'patch_orphan',
        ]),
        block_id: z.string().min(1).optional(),
        value: z.record(z.unknown()).default({}),
      })
      .strict(),
  })
  .strict();

export type EventFrame = z.infer<typeof EventFrameSchema>;

export const HelloSchema = z
  .object({
    protocol: z.literal(PROTOCOL_ID),
    kinds: z.array(z.string()).min(1).max(128),
    limits: z
      .object({
        max_blocks: z.number().int().positive().max(64).default(64),
        max_nesting: z.number().int().positive().max(4).default(4),
      })
      .default({}),
    resume: z.string().optional(),
  })
  .strict();

export type Hello = z.infer<typeof HelloSchema>;

export const WelcomeSchema = z
  .object({
    session_id: z.string().min(1),
    accepted_kinds: z.array(z.string()).min(1),
    server_limits: z.object({
      max_blocks: z.number().int().positive(),
      max_nesting: z.number().int().positive(),
    }),
    protocol: z.literal(PROTOCOL_ID),
  })
  .strict();

export type Welcome = z.infer<typeof WelcomeSchema>;

export function makeHello(extraKinds: string[] = []): Hello {
  return {
    protocol: PROTOCOL_ID,
    kinds: [...CORE_KINDS, ...extraKinds],
    limits: { max_blocks: 64, max_nesting: 4 },
  };
}

export function intersectKinds(client: string[], server: readonly string[]): string[] {
  const set = new Set(server);
  return client.filter((k) => set.has(k));
}

export function validateRenderFrame(
  frame: unknown,
  acceptedKinds?: string[],
): { ok: true; frame: RenderFrame } | { ok: false; error: string } {
  const parsed = RenderFrameSchema.safeParse(frame);
  if (!parsed.success) {
    return { ok: false, error: parsed.error.message };
  }
  const f = parsed.data;
  const allowed = new Set(acceptedKinds ?? CORE_KINDS);
  for (const block of f.blocks) {
    const err = validateBlock(block, allowed, 0);
    if (err) return { ok: false, error: err };
  }
  return { ok: true, frame: f };
}

function validateBlock(block: Block, allowed: Set<string>, depth: number): string | null {
  if (depth > 4) return 'nesting depth exceeded';
  if (!allowed.has(block.kind)) return `kind ${block.kind} not accepted`;
  if (block.kind !== 'text@1') {
    if (!block.fallback) return 'fallback_missing';
    if (block.fallback.kind !== 'text@1') return 'fallback must be text@1';
  }
  if (block.kind === 'text@1' && typeof block.body.md !== 'string') {
    return 'text@1 body.md required';
  }
  if (block.kind === 'group@1') {
    const kids = block.body.blocks;
    if (!Array.isArray(kids)) return 'group@1 blocks required';
    for (const child of kids) {
      const err = validateBlock(child as Block, allowed, depth + 1);
      if (err) return err;
    }
  }
  return null;
}

export function validateEventFrame(
  frame: unknown,
): { ok: true; frame: EventFrame } | { ok: false; error: string } {
  const parsed = EventFrameSchema.safeParse(frame);
  if (!parsed.success) return { ok: false, error: parsed.error.message };
  const f = parsed.data;
  if (f.event.type === 'message') {
    if (f.event.block_id) return { ok: false, error: 'message must not have block_id' };
    if (typeof f.event.value.text !== 'string' || !String(f.event.value.text).trim()) {
      return { ok: false, error: 'message text required' };
    }
  } else if (!f.event.block_id) {
    return { ok: false, error: 'event_orphan_block' };
  }
  return { ok: true, frame: f };
}

export function textFrame(turnId: string, frameId: string, md: string): RenderFrame {
  return {
    frame: 'render',
    frame_id: frameId,
    mode: 'append',
    turn_id: turnId,
    blocks: [
      {
        block_id: 'b0',
        kind: 'text@1',
        body: { md },
      },
    ],
  };
}

export function flattenToText(frame: RenderFrame): string {
  return frame.blocks.map(blockToText).join('\n\n');
}

function blockToText(b: Block): string {
  switch (b.kind) {
    case 'text@1':
      return String(b.body.md ?? '');
    case 'status@1':
      return String(b.body.label ?? '');
    case 'choice@1': {
      const opts = Array.isArray(b.body.options) ? b.body.options : [];
      const labels = opts
        .map((o) => (o && typeof o === 'object' && 'label' in o ? String((o as { label: unknown }).label) : ''))
        .filter(Boolean);
      return `Choose: ${labels.join(' | ')}`;
    }
    case 'confirm@1':
      return String(b.body.prompt ?? 'Confirm?');
    default:
      return b.fallback ? blockToText(b.fallback) : `[${b.kind}]`;
  }
}

/** Resolve which block to show: preferred kind if accepted, else fallback text. */
export function resolveBlockForClient(
  block: Block,
  acceptedKinds: string[],
): { block: Block; degraded: boolean } {
  if (acceptedKinds.includes(block.kind)) {
    return { block, degraded: false };
  }
  if (block.fallback) {
    return { block: block.fallback, degraded: true };
  }
  return {
    block: {
      block_id: `${block.block_id}_fb`,
      kind: 'text@1',
      body: { md: `[unsupported ${block.kind}]` },
    },
    degraded: true,
  };
}

/** Minimal markdown → safe HTML (subset R-17): no raw HTML, no images. */
export function renderMarkdownSubset(md: string): string {
  let s = escapeHtml(md);
  s = s.replace(/```(\w*)\n([\s\S]*?)```/g, (_m, _lang, code) => `<pre><code>${code.trim()}</code></pre>`);
  s = s.replace(/`([^`]+)`/g, '<code>$1</code>');
  s = s.replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>');
  s = s.replace(/\*([^*]+)\*/g, '<em>$1</em>');
  s = s.replace(/^### (.+)$/gm, '<h3>$1</h3>');
  s = s.replace(/^## (.+)$/gm, '<h2>$1</h2>');
  s = s.replace(/^# (.+)$/gm, '<h1>$1</h1>');
  s = s.replace(/^> (.+)$/gm, '<blockquote>$1</blockquote>');
  s = s.replace(/^- (.+)$/gm, '<li>$1</li>');
  s = s.replace(/(<li>.*<\/li>\n?)+/g, (m) => `<ul>${m}</ul>`);
  s = s.replace(
    /\[([^\]]+)\]\((https:\/\/[^)]+)\)/g,
    '<a href="$2" rel="noopener noreferrer" target="_blank">$1</a>',
  );
  s = s
    .split(/\n\n+/)
    .map((p) => (p.startsWith('<') ? p : `<p>${p.replace(/\n/g, '<br/>')}</p>`))
    .join('');
  return s;
}

function escapeHtml(s: string): string {
  return s
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
}

export function messageEvent(turnId: string, frameId: string, text: string): EventFrame {
  return {
    frame: 'event',
    frame_id: frameId,
    turn_id: turnId,
    event: { type: 'message', value: { text } },
  };
}
