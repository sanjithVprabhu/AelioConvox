import type {
  FlowDefinition,
  FunctionDefinition,
  InvocationContext,
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
  /** Client-declared assistant persona, when the SDK registered one. */
  getPersona?(): string | null;
  invoke(
    functionName: string,
    args: Record<string, unknown>,
    context: InvocationContext,
  ): Promise<SdkInvokeResult>;
}