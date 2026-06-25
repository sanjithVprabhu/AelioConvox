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

function getScriptDataset() {
  const script = document.currentScript as HTMLScriptElement | null;
  return {
    customerId: script?.dataset.customerId ?? 'demo-user',
    serverUrl: script?.dataset.serverUrl ?? window.location.origin,
  };
}

function ChatWidget() {
  const { customerId, serverUrl } = useMemo(() => getScriptDataset(), []);
  const [open, setOpen] = useState(false);
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [input, setInput] = useState('');
  const [typing, setTyping] = useState(false);
  const [connected, setConnected] = useState(false);
  const socketRef = useRef<WebSocket | null>(null);
  const listRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    const wsUrl = new URL('/widget/ws', serverUrl);
    wsUrl.protocol = wsUrl.protocol === 'https:' ? 'wss:' : 'ws:';
    const socket = new WebSocket(wsUrl);
    socketRef.current = socket;

    socket.onopen = () => {
      socket.send(JSON.stringify({ type: 'init', customerId }));
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
  }, [customerId, serverUrl]);

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
            <strong>Chat with us</strong>
            <button type="button" class="aelio-close" onClick={() => setOpen(false)}>
              ×
            </button>
          </div>
          <div class="aelio-messages" ref={listRef}>
            {messages.length === 0 ? (
              <div class="aelio-hint">Ask about your order status to try the SDK tool call.</div>
            ) : null}
            {messages.map((message) => (
              <div key={`${message.role}-${message.content}`} class={`aelio-msg aelio-${message.role}`}>
                {message.content}
              </div>
            ))}
            {typing ? <div class="aelio-typing">Aelio is typing…</div> : null}
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
              placeholder={connected ? 'Type a message…' : 'Connecting…'}
              disabled={!connected}
            />
            <button type="button" onClick={sendMessage} disabled={!connected || !input.trim()}>
              Send
            </button>
          </div>
        </div>
      ) : null}
      <button type="button" class="aelio-launcher" onClick={() => setOpen((value) => !value)}>
        {open ? 'Close' : 'Chat'}
      </button>
      <style>{`
        .aelio-widget { position: fixed; right: 20px; bottom: 20px; z-index: 99999; font-family: Inter, system-ui, sans-serif; }
        .aelio-launcher { background: #111827; color: #fff; border: none; border-radius: 999px; padding: 12px 18px; cursor: pointer; box-shadow: 0 8px 24px rgba(0,0,0,.2); }
        .aelio-panel { width: 340px; height: 460px; background: #fff; border-radius: 16px; box-shadow: 0 16px 40px rgba(0,0,0,.18); display: flex; flex-direction: column; overflow: hidden; margin-bottom: 12px; }
        .aelio-header { display: flex; justify-content: space-between; align-items: center; padding: 14px 16px; border-bottom: 1px solid #e5e7eb; }
        .aelio-close { background: transparent; border: none; font-size: 20px; cursor: pointer; }
        .aelio-messages { flex: 1; overflow-y: auto; padding: 12px; display: flex; flex-direction: column; gap: 8px; background: #f9fafb; }
        .aelio-msg { max-width: 85%; padding: 10px 12px; border-radius: 12px; line-height: 1.4; font-size: 14px; white-space: pre-wrap; }
        .aelio-user { align-self: flex-end; background: #111827; color: #fff; }
        .aelio-assistant { align-self: flex-start; background: #fff; border: 1px solid #e5e7eb; color: #111827; }
        .aelio-hint, .aelio-typing { color: #6b7280; font-size: 13px; }
        .aelio-input-row { display: flex; gap: 8px; padding: 12px; border-top: 1px solid #e5e7eb; }
        .aelio-input-row input { flex: 1; border: 1px solid #d1d5db; border-radius: 10px; padding: 10px 12px; }
        .aelio-input-row button { background: #111827; color: #fff; border: none; border-radius: 10px; padding: 0 14px; cursor: pointer; }
      `}</style>
    </div>
  );
}

function mount() {
  const root = document.createElement('div');
  root.id = 'aelio-widget-root';
  document.body.appendChild(root);
  render(<ChatWidget />, root);
}

if (document.readyState === 'loading') {
  document.addEventListener('DOMContentLoaded', mount);
} else {
  mount();
}