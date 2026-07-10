import { render } from 'preact';
import { useEffect, useMemo, useRef, useState } from 'preact/hooks';

type ChatMessage = {
  role: 'user' | 'assistant';
  content: string;
};

type ServerMessage =
  | { type: 'ready'; customerId: string }
  | { type: 'message'; role: 'assistant'; content: string }
  | { type: 'confirmation'; prompt: string; turnId?: string }
  | { type: 'typing'; active: boolean }
  | { type: 'error'; message: string; code?: string };

type ConnectionStatus = 'connecting' | 'connected' | 'reconnecting' | 'failed';

// How long to wait for the server's `ready` frame before declaring the handshake
// failed (covers a reachable socket that never completes init — e.g. wrong path).
const HANDSHAKE_TIMEOUT_MS = 8000;
const MAX_MESSAGE_LENGTH = 4096;
// Stop auto-reconnecting after this many consecutive failures.
const MAX_RECONNECT_ATTEMPTS = 10;

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
  const [status, setStatus] = useState<ConnectionStatus>('connecting');
  const [statusDetail, setStatusDetail] = useState('');
  const [pendingConfirmationIndex, setPendingConfirmationIndex] = useState<number | null>(null);
  const socketRef = useRef<WebSocket | null>(null);
  const listRef = useRef<HTMLDivElement | null>(null);
  // Set by the connection effect; the Retry button calls it to restart from scratch.
  const retryRef = useRef<() => void>(() => {});
  const connected = status === 'connected';
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
    let disposed = false;
    let attempt = 0;
    let retryTimer: ReturnType<typeof setTimeout> | null = null;
    let handshakeTimer: ReturnType<typeof setTimeout> | null = null;
    // An error frame the server sends just before closing (e.g. origin rejection),
    // stashed so onclose can report *why* instead of a generic drop.
    let pendingError: { message: string; code?: string } | null = null;

    const clearTimers = () => {
      if (retryTimer) {
        clearTimeout(retryTimer);
        retryTimer = null;
      }
      if (handshakeTimer) {
        clearTimeout(handshakeTimer);
        handshakeTimer = null;
      }
    };

    const scheduleReconnect = () => {
      if (disposed) {
        return;
      }
      if (attempt >= MAX_RECONNECT_ATTEMPTS) {
        setStatus('failed');
        setStatusDetail('Unable to reach the server. Check your connection, then retry.');
        return;
      }
      // Exponential backoff capped at 30s, with jitter to avoid thundering herds.
      const delay = Math.min(30_000, 1_000 * 2 ** attempt) + Math.floor(Math.random() * 1_000);
      attempt += 1;
      setStatus('reconnecting');
      setStatusDetail(`Reconnecting (attempt ${attempt})…`);
      retryTimer = setTimeout(connect, delay);
    };

    function connect() {
      if (disposed) {
        return;
      }
      clearTimers();
      pendingError = null;
      let ready = false;
      setStatus((prev) => (prev === 'reconnecting' ? 'reconnecting' : 'connecting'));

      // Close any previous socket before opening a new one (BUG-037).
      if (socketRef.current && socketRef.current.readyState <= WebSocket.OPEN) {
        try {
          socketRef.current.close();
        } catch {
          /* ignore */
        }
      }

      let wsUrl: URL;
      try {
        wsUrl = new URL('/widget/ws', options.serverUrl);
      } catch {
        setStatus('failed');
        setStatusDetail(`Invalid server URL: ${options.serverUrl}`);
        return;
      }
      wsUrl.protocol = wsUrl.protocol === 'https:' ? 'wss:' : 'ws:';

      let socket: WebSocket;
      try {
        socket = new WebSocket(wsUrl);
      } catch {
        scheduleReconnect();
        return;
      }
      socketRef.current = socket;

      handshakeTimer = setTimeout(() => {
        pendingError = {
          message: 'No response from server — check the server URL and that it allows this origin.',
        };
        try {
          socket.close();
        } catch {
          /* already closed */
        }
      }, HANDSHAKE_TIMEOUT_MS);

      socket.onopen = () => {
        socket.send(JSON.stringify(initPayload));
      };

      socket.onmessage = (event) => {
        let data: ServerMessage;
        try {
          data = JSON.parse(event.data as string) as ServerMessage;
        } catch {
          return;
        }
        if (data.type === 'ready') {
          ready = true;
          clearTimers();
          attempt = 0;
          setStatus('connected');
          setStatusDetail('');
          return;
        }
        if (data.type === 'typing') {
          setTyping(data.active);
          return;
        }
        if (data.type === 'message') {
          setPendingConfirmationIndex(null);
          setMessages((prev) => [...prev, { role: 'assistant', content: data.content }]);
          return;
        }
        if (data.type === 'confirmation') {
          setMessages((prev) => {
            const next: ChatMessage[] = [...prev, { role: 'assistant', content: data.prompt }];
            setPendingConfirmationIndex(next.length - 1);
            return next;
          });
          return;
        }
        if (data.type === 'error') {
          // After the handshake, errors are per-turn failures — show them inline.
          // Before it, the error explains the imminent close — stash for onclose.
          if (ready) {
            setMessages((prev) => [...prev, { role: 'assistant', content: data.message }]);
          } else {
            pendingError = { message: data.message, code: data.code };
          }
        }
      };

      socket.onclose = (event) => {
        if (disposed) {
          return;
        }
        clearTimers();
        // A policy rejection (origin not allowed, auth) will not heal by retrying.
        const policyRejected = event.code === 1008 || pendingError?.code === 'origin_not_allowed';
        if (policyRejected) {
          setStatus('failed');
          setStatusDetail(pendingError?.message || event.reason || 'Connection rejected by the server.');
          return;
        }
        scheduleReconnect();
      };
    }

    retryRef.current = () => {
      attempt = 0;
      connect();
    };
    connect();

    return () => {
      disposed = true;
      clearTimers();
      socketRef.current?.close();
    };
  }, [initPayload, options.serverUrl]);

  useEffect(() => {
    listRef.current?.scrollTo({ top: listRef.current.scrollHeight, behavior: 'smooth' });
  }, [messages, typing, open]);

  const sendMessage = (contentOverride?: string) => {
    const content = (contentOverride ?? input).trim();
    if (!content || !socketRef.current || socketRef.current.readyState !== WebSocket.OPEN) {
      return;
    }
    if (content.length > MAX_MESSAGE_LENGTH) {
      return;
    }
    setMessages((prev) => [...prev, { role: 'user', content }]);
    socketRef.current.send(JSON.stringify({ type: 'message', content }));
    if (!contentOverride) {
      setInput('');
    }
    setPendingConfirmationIndex(null);
  };

  const respondToConfirmation = (answer: 'yes' | 'no') => {
    if (pendingConfirmationIndex === null) {
      return;
    }
    sendMessage(answer);
  };

  const statusText =
    status === 'connecting'
      ? 'Connecting…'
      : status === 'reconnecting'
        ? statusDetail || 'Reconnecting…'
        : status === 'failed'
          ? `Connection failed: ${statusDetail}`
          : '';
  const placeholder =
    status === 'connected'
      ? 'Type a message...'
      : status === 'failed'
        ? 'Not connected'
        : 'Connecting…';

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
          {status !== 'connected' ? (
            <div class={`aelio-status aelio-status-${status}`} role="status">
              <span class="aelio-status-dot" />
              <span class="aelio-status-text">{statusText}</span>
              {status === 'failed' ? (
                <button type="button" class="aelio-retry" onClick={() => retryRef.current()}>
                  Retry
                </button>
              ) : null}
            </div>
          ) : null}
          <div class="aelio-messages" ref={listRef}>
            {messages.length === 0 ? <div class="aelio-hint">{options.initialMessage}</div> : null}
            {messages.map((message, index) => (
              <div key={`${message.role}-${index}`} class={`aelio-msg aelio-${message.role}`}>
                {message.content}
                {pendingConfirmationIndex === index ? (
                  <div class="aelio-confirm-actions">
                    <button
                      type="button"
                      class="aelio-confirm-yes"
                      onClick={() => respondToConfirmation('yes')}
                    >
                      Confirm
                    </button>
                    <button
                      type="button"
                      class="aelio-confirm-no"
                      onClick={() => respondToConfirmation('no')}
                    >
                      Cancel
                    </button>
                  </div>
                ) : null}
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
              placeholder={placeholder}
              disabled={!connected}
              maxLength={MAX_MESSAGE_LENGTH}
            />
            <button type="button" onClick={() => sendMessage()} disabled={!connected || !input.trim()}>
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
        .aelio-confirm-actions { display: flex; gap: 8px; margin-top: 10px; }
        .aelio-confirm-yes, .aelio-confirm-no { border: none; border-radius: 8px; padding: 6px 12px; font-size: 13px; cursor: pointer; }
        .aelio-confirm-yes { background: #111827; color: #fff; }
        .aelio-confirm-no { background: #f3f4f6; color: #374151; }
        .aelio-hint, .aelio-typing { color: #6b7280; font-size: 13px; }
        .aelio-status { display: flex; align-items: center; gap: 8px; padding: 8px 14px; font-size: 12px; border-bottom: 1px solid #e5e7eb; }
        .aelio-status-dot { width: 8px; height: 8px; border-radius: 999px; flex: none; }
        .aelio-status-text { flex: 1; min-width: 0; overflow-wrap: anywhere; }
        .aelio-status-connecting, .aelio-status-reconnecting { background: #fffbeb; color: #92400e; }
        .aelio-status-connecting .aelio-status-dot, .aelio-status-reconnecting .aelio-status-dot { background: #f59e0b; }
        .aelio-status-failed { background: #fef2f2; color: #991b1b; }
        .aelio-status-failed .aelio-status-dot { background: #ef4444; }
        .aelio-retry { flex: none; background: #991b1b; color: #fff; border: none; border-radius: 8px; padding: 4px 10px; font-size: 12px; cursor: pointer; }
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
