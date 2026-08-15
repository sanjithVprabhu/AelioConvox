import type {
  AttributeDefinition,
  FlowDefinition,
  FunctionDefinition,
  InvocationContext,
  PipelineManifest,
  PolicyDefinition,
  StateDefinition,
} from '@aelio/protocol';

export type SdkInvokeResult = {
  ok: boolean;
  data?: unknown;
  error?: string;
  durationMs: number;
};

export interface SdkBridge {
  getFunctions(): FunctionDefinition[];
  getStates(): StateDefinition[];
  getPolicies(): PolicyDefinition[];
  getFlows(): FlowDefinition[];
  getPipelineManifest?(): PipelineManifest | null;
  getAttributes?(): AttributeDefinition[];
  /** Client-declared assistant persona, when the SDK registered one. */
  getPersona?(): string | null;
  /** Client-written product description grounding the harness planner. */
  getProductBrief?(): string | null;
  invoke(
    functionName: string,
    args: Record<string, unknown>,
    context: InvocationContext,
  ): Promise<SdkInvokeResult>;
}