import { render } from 'preact';
import { useEffect, useMemo, useRef, useState } from 'preact/hooks';

type ChatMessage = {
  role: 'user' | 'assistant';
  content: string;
};

type ServerMessage =
  | { type: 'ready'; customerId: string }
  | { type: 'message'; role: 'assistant'; content: string }
  | { type: 'typing'; active: boolean }
  | { type: 'error'; message: string };

export type AelioChatOptions = {
  serverUrl: string;
  customerId: string;
  authToken?: string;
  email?: string;
  target?: HTMLElement | string;
  title?: string;
  launcherLabel?: string;
  initialMessage?: string;
};

export type AelioChatHandle = {
  root: HTMLElement;
  unmount: () => void;
};

type NormalizedOptions = Required<
  Pick<AelioChatOptions, 'serverUrl' | 'customerId' | 'title' | 'launcherLabel' | 'initialMessage'>
> &
  Pick<AelioChatOptions, 'authToken' | 'email'>;

declare global {
  interface Window {
    AelioChat?: {
      mount: typeof mountAelioChat;
      unmount: (handle: AelioChatHandle) => void;
    };
  }
}

function normalizeOptions(options: AelioChatOptions): NormalizedOptions {
  return {
    serverUrl: options.serverUrl.replace(/\/$/, ''),
    customerId: options.customerId,
    authToken: options.authToken,
    email: options.email,
    title: options.title ?? 'Chat with us',
    launcherLabel: options.launcherLabel ?? 'Chat',
    initialMessage: options.initialMessage ?? 'Ask a question and we will help from here.',
  };
}

function getTarget(target: AelioChatOptions['target']): HTMLElement {
  if (target instanceof HTMLElement) {
    return target;
  }
  if (typeof target === 'string') {
    const node = document.querySelector<HTMLElement>(target);
    if (!node) {
      throw new Error(`Aelio chat target not found: ${target}`);
    }
    return node;
  }
  const root = document.createElement('div');
  root.id = 'aelio-widget-root';
  document.body.appendChild(root);
  return root;
}

function getScriptOptions(): AelioChatOptions | null {
  const script = document.currentScript as HTMLScriptElement | null;
  if (!script) {
    return null;
  }
  return {
    serverUrl: script.dataset.serverUrl ?? window.location.origin,
    customerId: script.dataset.customerId ?? 'demo-user',
    authToken: script.dataset.authToken,
    email: script.dataset.email,
    title: script.dataset.title,
    launcherLabel: script.dataset.launcherLabel,
    initialMessage: script.dataset.initialMessage,
  };
}

function ChatWidget({ options }: { options: NormalizedOptions }) {
  const [open, setOpen] = useState(false);
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [input, setInput] = useState('');
  const [typing, setTyping] = useState(false);
  const [connected, setConnected] = useState(false);
  const socketRef = useRef<WebSocket | null>(null);
  const listRef = useRef<HTMLDivElement | null>(null);
  const initPayload = useMemo(
    () => ({
      type: 'init' as const,
      customerId: options.customerId,
      authToken: options.authToken,
      email: options.email,
    }),
    [options.authToken, options.customerId, options.email],
  );

  useEffect(() => {
    const wsUrl = new URL('/widget/ws', options.serverUrl);
    wsUrl.protocol = wsUrl.protocol === 'https:' ? 'wss:' : 'ws:';
    const socket = new WebSocket(wsUrl);
    socketRef.current = socket;

    socket.onopen = () => {
      socket.send(JSON.stringify(initPayload));
    };

    socket.onmessage = (event) => {
      const data = JSON.parse(event.data as string) as ServerMessage;
      if (data.type === 'ready') {
        setConnected(true);
        return;
      }
      if (data.type === 'typing') {
        setTyping(data.active);
        return;
      }
      if (data.type === 'message') {
        setMessages((prev) => [...prev, { role: 'assistant', content: data.content }]);
        return;
      }
      if (data.type === 'error') {
        setMessages((prev) => [...prev, { role: 'assistant', content: data.message }]);
      }
    };

    socket.onclose = () => setConnected(false);

    return () => socket.close();
  }, [initPayload, options.serverUrl]);

  useEffect(() => {
    listRef.current?.scrollTo({ top: listRef.current.scrollHeight, behavior: 'smooth' });
  }, [messages, typing, open]);

  const sendMessage = () => {
    const content = input.trim();
    if (!content || !socketRef.current || socketRef.current.readyState !== WebSocket.OPEN) {
      return;
    }
    setMessages((prev) => [...prev, { role: 'user', content }]);
    socketRef.current.send(JSON.stringify({ type: 'message', content }));
    setInput('');
  };

  return (
    <div class="aelio-widget">
      {open ? (
        <div class="aelio-panel">
          <div class="aelio-header">
            <strong>{options.title}</strong>
            <button type="button" class="aelio-close" onClick={() => setOpen(false)} aria-label="Close chat">
              x
            </button>
          </div>
          <div class="aelio-messages" ref={listRef}>
            {messages.length === 0 ? <div class="aelio-hint">{options.initialMessage}</div> : null}
            {messages.map((message, index) => (
              <div key={`${message.role}-${index}`} class={`aelio-msg aelio-${message.role}`}>
                {message.content}
              </div>
            ))}
            {typing ? <div class="aelio-typing">Aelio is typing...</div> : null}
          </div>
          <div class="aelio-input-row">
            <input
              value={input}
              onInput={(event) => setInput((event.target as HTMLInputElement).value)}
              onKeyDown={(event) => {
                if (event.key === 'Enter') {
                  sendMessage();
                }
              }}
              placeholder={connected ? 'Type a message...' : 'Connecting...'}
              disabled={!connected}
            />
            <button type="button" onClick={sendMessage} disabled={!connected || !input.trim()}>
              Send
            </button>
          </div>
        </div>
      ) : null}
      <button type="button" class="aelio-launcher" onClick={() => setOpen((value) => !value)}>
        {open ? 'Close' : options.launcherLabel}
      </button>
      <style>{`
        .aelio-widget { position: fixed; right: 20px; bottom: 20px; z-index: 99999; font-family: Inter, system-ui, sans-serif; }
        .aelio-launcher { background: #111827; color: #fff; border: none; border-radius: 999px; min-width: 72px; min-height: 44px; padding: 12px 18px; cursor: pointer; box-shadow: 0 8px 24px rgba(0,0,0,.2); }
        .aelio-panel { width: min(340px, calc(100vw - 32px)); height: min(460px, calc(100vh - 112px)); background: #fff; border-radius: 16px; box-shadow: 0 16px 40px rgba(0,0,0,.18); display: flex; flex-direction: column; overflow: hidden; margin-bottom: 12px; }
        .aelio-header { display: flex; justify-content: space-between; align-items: center; gap: 12px; padding: 14px 16px; border-bottom: 1px solid #e5e7eb; color: #111827; }
        .aelio-header strong { min-width: 0; overflow-wrap: anywhere; font-size: 15px; }
        .aelio-close { background: transparent; border: none; font-size: 20px; line-height: 1; cursor: pointer; color: #374151; }
        .aelio-messages { flex: 1; overflow-y: auto; padding: 12px; display: flex; flex-direction: column; gap: 8px; background: #f9fafb; }
        .aelio-msg { max-width: 85%; padding: 10px 12px; border-radius: 12px; line-height: 1.4; font-size: 14px; white-space: pre-wrap; overflow-wrap: anywhere; }
        .aelio-user { align-self: flex-end; background: #111827; color: #fff; }
        .aelio-assistant { align-self: flex-start; background: #fff; border: 1px solid #e5e7eb; color: #111827; }
        .aelio-hint, .aelio-typing { color: #6b7280; font-size: 13px; }
        .aelio-input-row { display: flex; gap: 8px; padding: 12px; border-top: 1px solid #e5e7eb; }
        .aelio-input-row input { flex: 1; min-width: 0; border: 1px solid #d1d5db; border-radius: 10px; padding: 10px 12px; font-size: 14px; }
        .aelio-input-row button { background: #111827; color: #fff; border: none; border-radius: 10px; min-width: 58px; padding: 0 14px; cursor: pointer; }
        .aelio-input-row button:disabled, .aelio-input-row input:disabled { opacity: .6; cursor: not-allowed; }
        @media (max-width: 480px) {
          .aelio-widget { right: 16px; bottom: 16px; }
          .aelio-panel { width: calc(100vw - 32px); height: min(520px, calc(100vh - 96px)); }
        }
      `}</style>
    </div>
  );
}

export function mountAelioChat(options: AelioChatOptions): AelioChatHandle {
  if (typeof document === 'undefined') {
    throw new Error('Aelio chat can only be mounted in a browser environment');
  }
  const normalized = normalizeOptions(options);
  const root = getTarget(options.target);
  render(<ChatWidget options={normalized} />, root);
  return {
    root,
    unmount: () => {
      render(null, root);
      if (!options.target) {
        root.remove();
      }
    },
  };
}

export function unmountAelioChat(handle: AelioChatHandle): void {
  handle.unmount();
}

if (typeof window !== 'undefined' && typeof document !== 'undefined') {
  window.AelioChat = {
    mount: mountAelioChat,
    unmount: unmountAelioChat,
  };

  const scriptOptions = getScriptOptions();
  if (scriptOptions && (document.currentScript as HTMLScriptElement | null)?.dataset.autoMount !== 'false') {
    if (document.readyState === 'loading') {
      document.addEventListener('DOMContentLoaded', () => mountAelioChat(scriptOptions), { once: true });
    } else {
      mountAelioChat(scriptOptions);
    }
  }
}
