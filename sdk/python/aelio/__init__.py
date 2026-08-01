"""Aelio Python SDK — expose backend functions to the Aelio conversational runtime."""

from __future__ import annotations

import asyncio
import json
import logging
import time
from dataclasses import dataclass
from typing import Any, Awaitable, Callable, Dict, List, Optional

import websockets

__version__ = "0.1.0"

HEARTBEAT_INTERVAL_MS = 30_000
HEARTBEAT_TIMEOUT_MS = 60_000
DEFAULT_SDK_PATH = "/sdk"

logger = logging.getLogger("aelio")

Handler = Callable[[Dict[str, Any], Dict[str, Any]], Awaitable[Any]]
SendHandler = Callable[[Dict[str, Any]], Awaitable[None]]


@dataclass
class FunctionSchema:
    description: str
    params: Dict[str, Any]
    safety: str
    intent: Optional[str] = None
    output: Optional[Dict[str, Dict[str, Any]]] = None
    output_role: Optional[str] = None


@dataclass
class StateSchema:
    description: str
    allowed_tools: Optional[List[str]] = None
    blocked_tools: Optional[List[str]] = None
    guards: Optional[Dict[str, Any]] = None
    transitions: Optional[List[Dict[str, Any]]] = None


@dataclass
class PolicySchema:
    description: str
    severity: str = "soft"
    aelio: Optional[Dict[str, Any]] = None


@dataclass
class FlowSchema:
    state: str
    description: str
    steps: Dict[str, Dict[str, Any]]


@dataclass
class ListenOptions:
    secret: str
    url: str = "ws://127.0.0.1:3010"
    sdk_version: str = __version__


class Aelio:
    """Outbound WebSocket SDK — dial out to Aelio; no inbound ports required."""

    def __init__(
        self,
        secret: Optional[str] = None,
        url: str = "ws://127.0.0.1:3010",
        sdk_version: str = __version__,
    ) -> None:
        self._default_secret = secret
        self._default_url = url.rstrip("/")
        self.sdk_version = sdk_version
        self.handlers: Dict[str, tuple[Handler, FunctionSchema]] = {}
        self.states: Dict[str, StateSchema] = {}
        self.policies: Dict[str, PolicySchema] = {}
        self.flows: Dict[str, FlowSchema] = {}
        self._send_handler: Optional[SendHandler] = None
        self._persona: Optional[str] = None
        self._product_brief: Optional[str] = None
        self._ws = None
        self._last_pong_at = int(time.time() * 1000)
        self._should_run = False
        self._listen_opts: Optional[ListenOptions] = None

    def state(
        self,
        state_id: str,
        description: str,
        *,
        allowed_tools: Optional[List[str]] = None,
        blocked_tools: Optional[List[str]] = None,
        guards: Optional[Dict[str, Any]] = None,
        transitions: Optional[List[Dict[str, Any]]] = None,
    ) -> None:
        self.states[state_id] = StateSchema(
            description=description,
            allowed_tools=allowed_tools,
            blocked_tools=blocked_tools,
            guards=guards,
            transitions=transitions,
        )

    def policy(
        self,
        policy_id: str,
        description: str,
        severity: str = "soft",
        aelio: Optional[Dict[str, Any]] = None,
    ) -> None:
        self.policies[policy_id] = PolicySchema(
            description=description, severity=severity, aelio=aelio
        )

    def flow(self, flow_id: str, state: str, description: str, steps: Dict[str, Dict[str, Any]]) -> None:
        self.flows[flow_id] = FlowSchema(state=state, description=description, steps=steps)

    async def set_customer_state(
        self, customer_id: str, state_id: str, reason: Optional[str] = None
    ) -> None:
        payload: Dict[str, Any] = {
            "type": "set_state",
            "customerId": customer_id,
            "stateId": state_id,
        }
        if reason is not None:
            payload["reason"] = reason
        await self._send(payload)

    def expose(
        self,
        name: str,
        description: str,
        params: Dict[str, Any],
        safety: str,
        intent: Optional[str] = None,
        output: Optional[Dict[str, Dict[str, Any]]] = None,
        output_role: Optional[str] = None,
    ):
        def decorator(func: Handler):
            self.handlers[name] = (
                func,
                FunctionSchema(
                    description=description,
                    params=params,
                    safety=safety,
                    intent=intent,
                    output=output,
                    output_role=output_role,
                ),
            )
            return func

        return decorator

    def persona(self, text: str) -> None:
        self._persona = text.strip()

    def describe(self, text: str) -> None:
        self._product_brief = text.strip()

    def on_send(self, handler: SendHandler) -> None:
        self._send_handler = handler

    async def ingest(
        self,
        channel: str,
        from_: str,
        text: str,
        message_id: Optional[str] = None,
        metadata: Optional[Dict[str, Any]] = None,
    ) -> None:
        payload: Dict[str, Any] = {
            "type": "ingest",
            "channel": channel,
            "from": from_,
            "text": text,
        }
        if message_id is not None:
            payload["messageId"] = message_id
        if metadata is not None:
            payload["metadata"] = metadata
        await self._send(payload)

    async def _send(self, payload: Dict[str, Any]) -> None:
        if self._ws is None:
            msg_type = payload.get("type")
            if msg_type not in ("result", "pong"):
                logger.warning("not connected — dropped %r message", msg_type)
            return
        await self._ws.send(json.dumps(payload))

    async def _send_register(self, websocket) -> None:
        functions = [
            {
                "name": name,
                "description": schema.description,
                "params": schema.params,
                "safety": schema.safety,
                **({"intent": schema.intent} if schema.intent else {}),
                **({"output": schema.output} if schema.output else {}),
                **({"outputRole": schema.output_role} if schema.output_role else {}),
            }
            for name, (_, schema) in self.handlers.items()
        ]
        states = []
        for state_id, schema in self.states.items():
            entry: Dict[str, Any] = {"id": state_id, "description": schema.description}
            if schema.allowed_tools:
                entry["allowedTools"] = schema.allowed_tools
            if schema.blocked_tools:
                entry["blockedTools"] = schema.blocked_tools
            if schema.guards and schema.guards.get("requires_fields"):
                entry["guards"] = {"requires_fields": schema.guards["requires_fields"]}
            if schema.transitions:
                entry["transitions"] = schema.transitions
            states.append(entry)
        policies = [
            {
                "id": policy_id,
                "description": schema.description,
                "severity": schema.severity,
                **({"aelio": schema.aelio} if schema.aelio is not None else {}),
            }
            for policy_id, schema in self.policies.items()
        ]
        flows = [
            {
                "id": flow_id,
                "state": schema.state,
                "description": schema.description,
                "steps": [
                    {
                        "id": step_id,
                        "goal": step["goal"],
                        **({"tool": step["tool"]} if step.get("tool") else {}),
                    }
                    for step_id, step in schema.steps.items()
                ],
            }
            for flow_id, schema in self.flows.items()
        ]
        payload: Dict[str, Any] = {
            "type": "register",
            "sdkVersion": self.sdk_version,
            "language": "python",
            "functions": functions,
            "canSend": self._send_handler is not None,
        }
        if self._persona:
            payload["persona"] = self._persona
        if self._product_brief:
            payload["productBrief"] = self._product_brief
        if states:
            payload["states"] = states
        if policies:
            payload["policies"] = policies
        if flows:
            payload["flows"] = flows
        await websocket.send(json.dumps(payload))

    async def _heartbeat(self, websocket) -> None:
        while True:
            await asyncio.sleep(HEARTBEAT_INTERVAL_MS / 1000)
            if int(time.time() * 1000) - self._last_pong_at > HEARTBEAT_TIMEOUT_MS:
                await websocket.close()
                return

    async def _handle_invoke(self, websocket, message: Dict[str, Any]) -> None:
        started = int(time.time() * 1000)
        fn_name = message.get("function")
        if fn_name not in self.handlers:
            await websocket.send(
                json.dumps(
                    {
                        "type": "result",
                        "id": message["id"],
                        "ok": False,
                        "error": {
                            "code": "FUNCTION_NOT_FOUND",
                            "message": f'Function "{fn_name}" is not registered',
                            "retryable": False,
                        },
                        "durationMs": int(time.time() * 1000) - started,
                    }
                )
            )
            return

        handler, _ = self.handlers[fn_name]
        try:
            data = await handler(message.get("args", {}), message.get("context", {}))
            payload: Dict[str, Any] = {
                "type": "result",
                "id": message["id"],
                "ok": True,
                "data": data,
                "durationMs": int(time.time() * 1000) - started,
            }
        except Exception as exc:  # noqa: BLE001 — surface to Aelio as handler error
            payload = {
                "type": "result",
                "id": message["id"],
                "ok": False,
                "error": {
                    "code": "HANDLER_ERROR",
                    "message": str(exc),
                    "retryable": False,
                },
                "durationMs": int(time.time() * 1000) - started,
            }
        await websocket.send(json.dumps(payload))

    async def _handle_send(self, websocket, message: Dict[str, Any]) -> None:
        started = int(time.time() * 1000)
        if self._send_handler is None:
            await websocket.send(
                json.dumps(
                    {
                        "type": "result",
                        "id": message["id"],
                        "ok": False,
                        "error": {
                            "code": "NO_SEND_HANDLER",
                            "message": "No on_send handler is registered",
                        },
                        "durationMs": int(time.time() * 1000) - started,
                    }
                )
            )
            return

        try:
            await self._send_handler(
                {
                    "channel": message.get("channel"),
                    "to": message.get("to"),
                    "content": message.get("content"),
                    "metadata": message.get("metadata"),
                }
            )
            await websocket.send(
                json.dumps(
                    {
                        "type": "result",
                        "id": message["id"],
                        "ok": True,
                        "durationMs": int(time.time() * 1000) - started,
                    }
                )
            )
        except Exception as exc:  # noqa: BLE001
            await websocket.send(
                json.dumps(
                    {
                        "type": "result",
                        "id": message["id"],
                        "ok": False,
                        "error": {
                            "code": "SEND_FAILED",
                            "message": str(exc),
                            "retryable": True,
                        },
                        "durationMs": int(time.time() * 1000) - started,
                    }
                )
            )

    async def listen(
        self,
        *,
        secret: Optional[str] = None,
        url: Optional[str] = None,
        sdk_version: Optional[str] = None,
    ) -> None:
        secret_val = secret or self._default_secret
        if not secret_val:
            raise ValueError("secret is required (listen(secret=...) or Aelio(secret=...))")
        self._listen_opts = ListenOptions(
            secret=secret_val,
            url=(url or self._default_url).rstrip("/"),
            sdk_version=sdk_version or self.sdk_version,
        )
        self.sdk_version = self._listen_opts.sdk_version
        self._should_run = True
        endpoint = f"{self._listen_opts.url}{DEFAULT_SDK_PATH}"
        backoff = 1
        while self._should_run:
            try:
                async with websockets.connect(
                    endpoint,
                    ping_interval=None,
                    additional_headers={"Authorization": f"Bearer {self._listen_opts.secret}"},
                ) as websocket:
                    self._ws = websocket
                    self._last_pong_at = int(time.time() * 1000)
                    await self._send_register(websocket)
                    heartbeat_task = asyncio.create_task(self._heartbeat(websocket))
                    try:
                        async for raw in websocket:
                            message = json.loads(raw)
                            msg_type = message.get("type")
                            if msg_type == "ping":
                                self._last_pong_at = int(time.time() * 1000)
                                await websocket.send(
                                    json.dumps({"type": "pong", "ts": message.get("ts")})
                                )
                            elif msg_type == "invoke":
                                await self._handle_invoke(websocket, message)
                            elif msg_type == "send":
                                await self._handle_send(websocket, message)
                            elif msg_type == "error":
                                logger.warning(
                                    "server error [%s]: %s",
                                    message.get("code"),
                                    message.get("message"),
                                )
                    finally:
                        heartbeat_task.cancel()
                        self._ws = None
                backoff = 1
            except Exception:  # noqa: BLE001 — reconnect loop
                self._ws = None
                if not self._should_run:
                    return
                await asyncio.sleep(backoff)
                backoff = min(30, backoff * 2)

    async def disconnect(self) -> None:
        self._should_run = False
        if self._ws is not None:
            await self._ws.close()
            self._ws = None

    def run(self, **kwargs: Any) -> None:
        asyncio.run(self.listen(**kwargs))


# Module-level singleton matching the Node SDK's `aelio` export.
aelio = Aelio()
