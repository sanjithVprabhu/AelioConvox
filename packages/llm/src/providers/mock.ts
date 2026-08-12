import type { ChatMessage, LLMCompleteOptions, LLMCompleteResult, LLMProvider, LLMToolCall } from '../types.js';

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

    // Agent-loop ReAct transport: no provider tools — emit JSON text actions.
    if (
      (opts.tools?.length ?? 0) === 0
      && (
        opts.telemetry?.purpose === 'agent_loop'
        || (opts.system ?? '').includes('Available tools')
        || (opts.system ?? '').includes('"actions"')
      )
    ) {
      return mockReactAgentComplete(opts);
    }

    let lastNaturalUserIndex = -1;
    opts.messages.forEach((message, index) => {
      if (
        message.role === 'user'
        && message.content.trim().length > 0
        && !message.content.trimStart().startsWith('{"kernel_notice"')
      ) {
        lastNaturalUserIndex = index;
      }
    });
    const currentTurnMessages = opts.messages.slice(lastNaturalUserIndex + 1);
    const hasToolResults = currentTurnMessages.some(
      (message) => message.toolResults && message.toolResults.length > 0,
    );
    const finishTool = opts.tools.find((tool) => tool.name === 'finish');
    if (hasToolResults) {
      // Approximate what a real LLM would say from the tools that ran — the
      // harness feeds executed tool calls in as assistant toolCalls, so the
      // synthesis reply reflects the action (e.g. a cancellation) rather than a
      // single canned string.
      const calledTools = currentTurnMessages
        .flatMap((message) => message.toolCalls ?? [])
        .map((call) => call.name.toLowerCase().replace(/[^a-z0-9]/g, ''));
      const hadToolError = currentTurnMessages
        .flatMap((message) => message.toolResults ?? [])
        .some((result) => {
          try {
            return Boolean((JSON.parse(result.content) as { error?: unknown }).error);
          } catch {
            return true;
          }
        });
      const toolResultText = currentTurnMessages
        .flatMap((message) => message.toolResults ?? [])
        .map((result) => result.content)
        .join('\n')
        .toLowerCase();
      const finalText = hadToolError
        ? 'I could not complete that action safely.'
        : calledTools.some((name) => name.includes('memorysearch'))
          ? toolResultText.includes('metric') && toolResultText.includes('imperial')
            ? 'I found conflicting unit preferences in the retrieved conversation history.'
            : toolResultText.includes('metric')
              ? 'Based on what I remember, you prefer metric units.'
              : toolResultText.includes('imperial')
                ? 'Based on what I remember, you prefer imperial units.'
                : 'I could not find a matching preference in our conversation history.'
        : calledTools.some((name) => name.includes('cancel'))
          ? 'Your order has been cancelled. Is there anything else I can help with?'
          : calledTools.some((name) => name.includes('listorder'))
            ? 'You have 3 orders: A123 (shipped), B456 (pending), and C789 (shipped).'
            : 'Your last order shipped today. Tracking: 1Z999AA10123456784';
      if (finishTool) {
        return {
          text: '',
          toolCalls: [{
            id: 'mock_finish_after_tools',
            name: 'finish',
            args: {
              message: finalText,
              status: hadToolError ? 'blocked' : 'completed',
              resolved_effect_ids: [],
              unresolved_effect_ids: [],
            },
          }],
          stopReason: 'tool_use',
        };
      }
      return {
        text: finalText,
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

    const normalizedToolName = (name: string) => name.toLowerCase().replace(/[^a-z0-9]/g, '');
    const explicitOrderId = content.match(/\b[a-z]\d{3,}\b/i)?.[0]?.toUpperCase() ?? 'last';
    const cancelTool = opts.tools.find((tool) =>
      normalizedToolName(tool.name).includes('cancelorder')
    );
    const memorySearchTool = opts.tools.find((tool) =>
      normalizedToolName(tool.name).includes('memorysearch')
    );
    if (
      memorySearchTool
      && (content.includes('what') || content.includes('remember'))
      && (content.includes('unit') || content.includes('prefer') || content.includes('memory'))
    ) {
      return {
        text: '',
        toolCalls: [{
          id: 'mock_tool_memory_search',
          name: memorySearchTool.name,
          args: { query: content, limit: 5 },
        }],
        stopReason: 'tool_use',
      };
    }
    if (cancelTool && content.includes('cancel')) {
      return {
        text: '',
        toolCalls: [
          {
            id: 'mock_tool_cancel',
            name: cancelTool.name,
            args: { orderId: explicitOrderId },
          },
        ],
        stopReason: 'tool_use',
      };
    }


    const listOrdersTool = opts.tools.find((tool) =>
      normalizedToolName(tool.name).includes('listorders')
    );
    if (
      listOrdersTool
      && (content.includes('list') || content.includes('show') || content.includes('my orders'))
    ) {
      return {
        text: '',
        toolCalls: [{
          id: 'mock_tool_list_orders',
          name: listOrdersTool.name,
          args: {},
        }],
        stopReason: 'tool_use',
      };
    }

    const orderTool = opts.tools.find((tool) =>
      normalizedToolName(tool.name).includes('getorderstatus')
    );
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
            name: orderTool.name,
            args: { orderId: explicitOrderId },
          },
        ],
        stopReason: 'tool_use',
      };
    }

    const greeting = 'Hi! I can help you check order status. Try asking "what is my order status?"';
    return finishTool
      ? {
          text: '',
          toolCalls: [{
            id: 'mock_finish_reply',
            name: 'finish',
            args: {
              message: greeting,
              status: 'completed',
              resolved_effect_ids: [],
              unresolved_effect_ids: [],
            },
          }],
          stopReason: 'tool_use',
        }
      : { text: greeting, toolCalls: [], stopReason: 'stop' };
  }
}

function reactJson(actions: Array<{ name: string; arguments: Record<string, unknown> }>, thought = '') {
  return {
    text: JSON.stringify({ thought, actions }),
    toolCalls: [] as LLMToolCall[],
    stopReason: 'stop' as const,
  };
}

function catalogToolNames(system: string): string[] {
  return [...system.matchAll(/^- ([a-zA-Z0-9_]+) \[v/gm)]
    .map((match) => match[1])
    .filter((name): name is string => Boolean(name));
}

function mockReactAgentComplete(opts: LLMCompleteOptions): LLMCompleteResult {
  const system = opts.system ?? '';
  const tools = catalogToolNames(system);
  const has = (name: string) => tools.some((tool) => tool.toLowerCase() === name.toLowerCase());
  const content = lastUserMessage(opts.messages);
  const observationText = [...opts.messages]
    .reverse()
    .find((message) => message.role === 'user' && message.content.startsWith('Observation:'))
    ?.content
    .toLowerCase() ?? '';

  if (observationText) {
    const hadError = observationText.includes('"error":{') || observationText.includes('"error": {');
    const message = hadError
      ? 'I could not complete that action safely.'
      : observationText.includes('metric')
        ? 'Based on what I remember, you prefer metric units.'
        : observationText.includes('cancel')
          ? 'Your order has been cancelled. Is there anything else I can help with?'
          : observationText.includes('ship') || observationText.includes('tracking')
            ? 'Your last order shipped today. Tracking: 1Z999AA10123456784'
            : 'Done.';
    if (has('finish')) {
      return reactJson([{
        name: 'finish',
        arguments: {
          message,
          status: hadError ? 'blocked' : 'completed',
          resolved_effect_ids: [],
          unresolved_effect_ids: [],
        },
      }]);
    }
    return { text: message, toolCalls: [], stopReason: 'stop' };
  }

  if (
    has('memory_search')
    && (content.includes('what') || content.includes('remember'))
    && (content.includes('unit') || content.includes('prefer') || content.includes('memory'))
  ) {
    return reactJson([{ name: 'memory_search', arguments: { query: content, limit: 5 } }]);
  }

  const cancelName = tools.find((name) => name.toLowerCase().includes('cancelorder'));
  if (cancelName && content.includes('cancel')) {
    const orderId = content.match(/\b[a-z]\d{3,}\b/i)?.[0]?.toUpperCase() ?? 'last';
    return reactJson([{ name: cancelName, arguments: { orderId } }]);
  }

  const listName = tools.find((name) => name.toLowerCase().includes('listorders'));
  if (listName && (content.includes('list') || content.includes('show') || content.includes('my orders'))) {
    return reactJson([{ name: listName, arguments: {} }]);
  }

  const orderName = tools.find((name) => name.toLowerCase().includes('getorderstatus'));
  if (
    orderName
    && !content.includes('cancel')
    && (content.includes('order') || content.includes('status') || content.includes('ship'))
  ) {
    const orderId = content.match(/\b[a-z]\d{3,}\b/i)?.[0]?.toUpperCase() ?? 'last';
    return reactJson([{ name: orderName, arguments: { orderId } }]);
  }

  const greeting = 'Hi! I can help you check order status. Try asking "what is my order status?"';
  if (has('finish')) {
    return reactJson([{
      name: 'finish',
      arguments: {
        message: greeting,
        status: 'completed',
        resolved_effect_ids: [],
        unresolved_effect_ids: [],
      },
    }]);
  }
  return { text: greeting, toolCalls: [], stopReason: 'stop' };
}
