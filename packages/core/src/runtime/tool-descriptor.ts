import type { FunctionDefinition } from '@aelio/protocol';

/** Text used for tool vector similarity: name + intent + description. */
export function buildToolDescriptor(tool: FunctionDefinition): string {
  return [tool.name, tool.intent ?? '', tool.description].filter(Boolean).join(' — ');
}
