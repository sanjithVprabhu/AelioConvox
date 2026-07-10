import type { ChatMessage, LLMCompleteOptions, LLMCompleteResult, LLMProvider } from './types.js';

function lastUserMessage(messages: ChatMessage[]): string {
  const message = [...messages].reverse().find((entry) => entry.role === 'user' && entry.content.trim());
  return message?.content.toLowerCase() ?? '';
}

export class MockProvider implements LLMProvider {
  async complete(opts: LLMCompleteOptions): Promise<LLMCompleteResult> {
    const purpose = opts.telemetry?.purpose;

    if (purpose === 'harness_router') {
      const content = lastUserMessage(opts.messages);
      const category =
        content.includes('checkout') || content.includes('buy') || content.includes('order')
          ? 'transaction'
          : content.includes('cancel')
            ? 'order'
            : 'general';
      return {
        text: JSON.stringify({
          intent: content || 'general inquiry',
          category,
        }),
        toolCalls: [],
        stopReason: 'stop',
        usage: { inputTokens: 10, outputTokens: 10 },
      };
    }

    if (
      purpose === 'harness_planner' ||
      purpose === 'harness_replan'
    ) {
      const content = lastUserMessage(opts.messages);
      const steps =
        content.includes('cancel')
          ? [{ tool_name: 'cancelOrder', input: { orderId: 'last' }, depends_on_step_index: [] }]
          : content.includes('order') || content.includes('status') || content.includes('ship')
            ? [{ tool_name: 'getOrderStatus', input: { orderId: 'last' }, depends_on_step_index: [] }]
            : [{ tool_name: 'getOrderStatus', input: { orderId: 'last' }, depends_on_step_index: [] }];

      return {
        text: '',
        toolCalls: [
          {
            id: 'mock_submit_plan',
            name: 'submit_plan',
            args: { steps },
          },
        ],
        stopReason: 'tool_use',
        usage: { inputTokens: 20, outputTokens: 20 },
      };
    }

    if (purpose === 'harness_synthesis') {
      return {
        text: 'Your request has been completed successfully.',
        toolCalls: [],
        stopReason: 'stop',
        usage: { inputTokens: 15, outputTokens: 15 },
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
