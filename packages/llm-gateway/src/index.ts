// @aelio/llm-gateway — a stateless multi-LLM "modem" the Rust brain dials for a model call.
// Rust decides *when and why*; this only *executes* the call and shapes the reply.
export {
  handleComplete,
  handleEmbed,
  GatewayError,
  IdempotencyCache,
  type GatewayRequest,
  type GatewayResponse,
  type GatewayDeps,
  type EmbedRequest,
  type EmbedResponse,
  type EmbedDeps,
} from './gateway.js';
export {
  resolveRoute,
  resolveEmbeddingRoute,
  type ResolvedRoute,
  type ResolvedEmbeddingRoute,
  type ProviderName,
} from './routing.js';
export { createGatewayServer } from './server.js';
