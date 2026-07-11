# @aelio/chat

Embeddable browser chat widget for Aelio.

Use this in your frontend app when you want a customer-facing chat window that connects to an Aelio server. This is separate from `@aelio/sdk`, which belongs in your backend and exposes your business functions to Aelio.

## Install

```bash
npm install @aelio/chat
```

## Use From Your App

```ts
import { mountAelioChat } from '@aelio/chat';

mountAelioChat({
  serverUrl: import.meta.env.VITE_AELIO_SERVER_URL,
  customerId: currentUser.id,
  authToken: currentUser.aelioChatToken,
  email: currentUser.email,
});
```

`authToken` is optional in local development, but production apps should pass a short-lived user/session token issued by your backend. Do not put `AELIO_SDK_SECRET` in frontend code.

## Script Tag

If you do not want to bundle the package, load the server-hosted script:

```html
<script
  src="https://your-aelio-server.com/widget.js"
  data-server-url="https://your-aelio-server.com"
  data-customer-id="customer_123"
  data-auth-token="short-lived-customer-token">
</script>
```

## Options

```ts
type AelioChatOptions = {
  serverUrl: string;
  customerId: string;
  authToken?: string;
  email?: string;
  target?: HTMLElement | string;
  title?: string;
  launcherLabel?: string;
  initialMessage?: string;
};
```

## API

- `mountAelioChat(options)` mounts the widget and returns a handle.
- `unmountAelioChat(handle)` removes a mounted widget.
- Script-tag users can call `window.AelioChat.mount(options)`.

## Chat history

On connect the widget receives prior user/assistant messages from the Aelio server
(in the `ready` frame's `history` field) and renders them automatically. Requires an
Aelio server that sends history on init (v0.1.2+ widget with a matching server build).

## Connection status & reconnect

The widget surfaces its connection state in the panel (a status dot + label) instead of sitting silently on "Connecting…":

- **Connecting / Reconnecting** — establishing the WebSocket. On an unexpected drop (network blip, server restart) it auto-reconnects with exponential backoff.
- **Connected** — ready to send.
- **Connection failed** — shows the reason and a **Retry** button. The two common causes:
  - **"Origin not allowed"** — the page's origin isn't in the server's `channels.web.allowed_origins`. Add it (e.g. `http://127.0.0.1:4173`) or use `"*"` for local dev. Origins are matched exactly, so `localhost` and `127.0.0.1` are distinct. This is a policy rejection, so the widget does **not** auto-retry.
  - **Handshake timeout** — no response within ~8s. Check `serverUrl`/`data-server-url`, that the server is reachable (`/health`), and that `channels.web.enabled` is true.
