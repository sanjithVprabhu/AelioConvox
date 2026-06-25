import type { ChatMessage, LLMCompleteOptions, LLMCompleteResult, LLMProvider } from './types.js';

function lastUserMessage(messages: ChatMessage[]): string {
  const message = [...messages].reverse().find((entry) => entry.role === 'user' && entry.content.trim());
  return message?.content.toLowerCase() ?? '';
}

export class MockProvider implements LLMProvider {
  async complete(opts: LLMCompleteOptions): Promise<LLMCompleteResult> {
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