import type { FunctionDefinition, InvocationContext } from '@aelio/protocol';

export type SdkInvokeResult = {
  ok: boolean;
  data?: unknown;
  error?: string;
  durationMs: number;
};

export interface SdkBridge {
  getFunctions(): FunctionDefinition[];
  invoke(
    functionName: string,
    args: Record<string, unknown>,
    context: InvocationContext,
  ): Promise<SdkInvokeResult>;
}