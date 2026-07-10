import type { ChatMessage, LLMCompleteOptions, LLMCompleteResult, LLMProvider } from './types.js';

function lastUserMessage(messages: ChatMessage[]): string {
  const message = [...messages].reverse().find((entry) => entry.role === 'user' && entry.content.trim());
  return message?.content.toLowerCase() ?? '';
}

/**
 * When the harness forces its planner tool (`emit_turn`), synthesize a plan from
 * the same keyword heuristics the free-form branches use, so the mock provider can
 * drive the full deep path (plan → execute → synthesize) in the phase tests.
 * Tool availability is read from the system prompt's tool cards, mirroring how a
 * real planner only knows the tools it was shown.
 */
function mockEmitTurn(content: string, system: string): Record<string, unknown> {
  if (content.includes('cancel') && system.includes('cancelorder')) {
    return {
      mode: 'plan',
      goal: 'Cancel the customer’s most recent order',
      instructions: [
        {
          id: 'a',
          capability: 'cancel the last order',
          tool: 'cancelOrder',
          args_hint: { orderId: 'last' },
        },
      ],
    };
  }

  if (
    (content.includes('order') || content.includes('status') || content.includes('ship')) &&
    system.includes('getorderstatus')
  ) {
    return {
      mode: 'plan',
      goal: 'Look up the status of the customer’s last order',
      instructions: [
        {
          id: 'a',
          capability: 'look up order status',
          tool: 'getOrderStatus',
          args_hint: { orderId: 'last' },
        },
      ],
    };
  }

  if (
    (content.includes('unit') || content.includes('metric') || content.includes('prefer')) &&
    system.includes('metric units')
  ) {
    return { mode: 'reply', text: 'Based on what I remember, you prefer metric units.' };
  }

  return {
    mode: 'reply',
    text: 'Hi! I can help you check order status. Try asking "what is my order status?"',
  };
}

export class MockProvider implements LLMProvider {
  async complete(opts: LLMCompleteOptions): Promise<LLMCompleteResult> {
    // A forced tool call must be honored regardless of content (structured output).
    if (opts.toolChoice?.type === 'tool') {
      const name = opts.toolChoice.name;
      const content = lastUserMessage(opts.messages);
      const system = opts.system?.toLowerCase() ?? '';
      const args =
        name === 'emit_turn'
          ? mockEmitTurn(content, system)
          : {};
      return {
        text: '',
        toolCalls: [{ id: `mock_forced_${name}`, name, args }],
        stopReason: 'tool_use',
      };
    }

    const hasToolResults = opts.messages.some(
      (message) => message.toolResults && message.toolResults.length > 0,
    );
    if (hasToolResults) {
      return {
        text: 'Your last order shipped today. Tracking: 1Z999AA10123456784',
        toolCalls: [],
        stopReason: 'stop',
      };
    }

    const content = lastUserMessage(opts.messages);
    const system = opts.system?.toLowerCase() ?? '';

    if (
      (content.includes('unit') || content.includes('metric') || content.includes('prefer')) &&
      system.includes('metric units')
    ) {
      return {
        text: 'Based on what I remember, you prefer metric units.',
        toolCalls: [],
        stopReason: 'stop',
      };
    }

    const cancelTool = opts.tools.find((tool) => tool.name === 'cancelOrder');
    if (cancelTool && content.includes('cancel')) {
      return {
        text: '',
        toolCalls: [
          {
            id: 'mock_tool_cancel',
            name: 'cancelOrder',
            args: { orderId: 'last' },
          },
        ],
        stopReason: 'tool_use',
      };
    }

    const orderTool = opts.tools.find((tool) => tool.name === 'getOrderStatus');
    if (
      orderTool &&
      !content.includes('cancel') &&
      (content.includes('order') || content.includes('status') || content.includes('ship'))
    ) {
      return {
        text: '',
        toolCalls: [
          {
            id: 'mock_tool_1',
            name: 'getOrderStatus',
            args: { orderId: 'last' },
          },
        ],
        stopReason: 'tool_use',
      };
    }

    return {
      text: 'Hi! I can help you check order status. Try asking "what is my order status?"',
      toolCalls: [],
      stopReason: 'stop',
    };
  }
}