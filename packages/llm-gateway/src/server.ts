// Minimal HTTP surface for the gateway. Deliberately dependency-light (node:http) so the
// "modem" has a tiny attack/ops surface. Exposes exactly two routes:
//
//   POST /v1/llm/complete   — run one closed-schema completion (see gateway.ts)
//   GET  /healthz           — liveness
//
// It holds no turn state, no policy, no tools. Auth to the gateway (optional) is a single
// shared bearer token in LLM_GATEWAY_TOKEN — this is the service's own token, never a tenant key.

import { createServer, type IncomingMessage, type ServerResponse } from 'node:http';
import {
  GatewayError,
  IdempotencyCache,
  handleComplete,
  handleEmbed,
  type EmbedRequest,
  type GatewayRequest,
} from './gateway.js';
import { resolveEmbeddingRoute, resolveRoute } from './routing.js';

const cache = new IdempotencyCache(Number(process.env.LLM_GATEWAY_IDEMPOTENCY_TTL_MS ?? 60_000));

function sendJson(res: ServerResponse, status: number, body: unknown): void {
  const payload = JSON.stringify(body);
  res.writeHead(status, { 'content-type': 'application/json' });
  res.end(payload);
}

async function readJsonBody(req: IncomingMessage, limitBytes = 1_000_000): Promise<unknown> {
  const chunks: Buffer[] = [];
  let size = 0;
  for await (const chunk of req) {
    size += chunk.length;
    if (size > limitBytes) throw new GatewayError(413, 'request body too large');
    chunks.push(chunk as Buffer);
  }
  if (size === 0) return {};
  try {
    return JSON.parse(Buffer.concat(chunks).toString('utf8'));
  } catch {
    throw new GatewayError(400, 'request body is not valid JSON');
  }
}

function authorized(req: IncomingMessage): boolean {
  const expected = process.env.LLM_GATEWAY_TOKEN;
  if (!expected) return true; // no token configured → open (dev)
  const header = req.headers.authorization ?? '';
  return header === `Bearer ${expected}`;
}

/** Structured, secret-free access log: never the prompt text. */
function logCall(req: GatewayRequest, provider: string, status: number, ms: number): void {
  const line = {
    at: new Date().toISOString(),
    request_id: req.request_id,
    spec_id: req.prompt?.spec_id,
    prompt_hash: req.prompt?.prompt_hash,
    model: req.model,
    provider,
    status,
    ms,
  };
  // eslint-disable-next-line no-console
  console.log(JSON.stringify(line));
}

export function createGatewayServer() {
  return createServer((req, res) => {
    void (async () => {
      const started = Date.now();
      try {
        if (req.method === 'GET' && req.url === '/healthz') {
          return sendJson(res, 200, { ok: true });
        }
        const isComplete = req.method === 'POST' && req.url === '/v1/llm/complete';
        const isEmbed = req.method === 'POST' && req.url === '/v1/llm/embed';
        if (!isComplete && !isEmbed) {
          return sendJson(res, 404, { error: 'not found' });
        }
        if (!authorized(req)) {
          return sendJson(res, 401, { error: 'unauthorized' });
        }

        if (isEmbed) {
          const body = (await readJsonBody(req)) as EmbedRequest;
          const response = await handleEmbed(body, { resolveRoute: resolveEmbeddingRoute });
          // Access log carries counts only — never the embedded text.
          // eslint-disable-next-line no-console
          console.log(
            JSON.stringify({
              at: new Date().toISOString(),
              route: 'embed',
              request_id: body.request_id,
              model: body.model,
              provider: response.provider,
              inputs: Array.isArray(body.input) ? body.input.length : 0,
              status: 200,
              ms: Date.now() - started,
            }),
          );
          return sendJson(res, 200, response);
        }

        const body = (await readJsonBody(req)) as GatewayRequest;
        const response = await handleComplete(body, { resolveRoute, cache });
        logCall(body, response.provider, 200, Date.now() - started);
        return sendJson(res, 200, response);
      } catch (error) {
        if (error instanceof GatewayError) {
          return sendJson(res, error.status, { error: error.message });
        }
        return sendJson(res, 500, { error: (error as Error).message });
      }
    })();
  });
}
