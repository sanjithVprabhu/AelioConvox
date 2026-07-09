import asyncio
import json
import time
from dataclasses import dataclass
from typing import Any, Awaitable, Callable, Dict, Optional

import websockets

HEARTBEAT_INTERVAL_MS = 30_000
HEARTBEAT_TIMEOUT_MS = 60_000
DEFAULT_SDK_PATH = "/sdk"


Handler = Callable[[Dict[str, Any], Dict[str, Any]], Awaitable[Any]]
SendHandler = Callable[[Dict[str, Any]], Awaitable[None]]


@dataclass
class FunctionSchema:
    description: str
    params: Dict[str, Any]
    safety: str
    intent: Optional[str] = None


@dataclass
class StateSchema:
    description: str
    allowed_tools: Optional[list[str]] = None
    blocked_tools: Optional[list[str]] = None


@dataclass
class PolicySchema:
    description: str
    severity: str = "soft"


@dataclass
class FlowSchema:
    state: str
    description: str
    steps: Dict[str, Dict[str, Any]]


class Aelio:
    def __init__(self, secret: str, url: str = "ws://127.0.0.1:3000", sdk_version: str = "0.1.0") -> None:
        self.secret = secret
        self.url = url.rstrip("/")
        self.sdk_version = sdk_version
        self.handlers: Dict[str, tuple[Handler, FunctionSchema]] = {}
        self.states: Dict[str, StateSchema] = {}
        self.policies: Dict[str, PolicySchema] = {}
        self.flows: Dict[str, FlowSchema] = {}
        self._send_handler: Optional[SendHandler] = None
        self._persona: Optional[str] = None
        self._ws = None
        self._last_pong_at = int(time.time() * 1000)

    def state(self, state_id: str, description: str, allowed_tools: Optional[list[str]] = None, blocked_tools: Optional[list[str]] = None) -> None:
        self.states[state_id] = StateSchema(description=description, allowed_tools=allowed_tools, blocked_tools=blocked_tools)

    def policy(self, policy_id: str, description: str, severity: str = "soft") -> None:
        self.policies[policy_id] = PolicySchema(description=description, severity=severity)

    def flow(self, flow_id: str, state: str, description: str, steps: Dict[str, Dict[str, Any]]) -> None:
        self.flows[flow_id] = FlowSchema(state=state, description=description, steps=steps)

    async def set_customer_state(self, customer_id: str, state_id: str, reason: Optional[str] = None) -> None:
        if self._ws is None:
            raise RuntimeError("Aelio SDK is not connected yet; call listen() first")
        payload: Dict[str, Any] = {"type": "set_state", "customerId": customer_id, "stateId": state_id}
        if reason is not None:
            payload["reason"] = reason
        await self._ws.send(json.dumps(payload))

    async def set_flow_progress(
        self,
        customer_id: str,
        flow_id: str,
        step_index: int,
        completed_steps: Optional[list[str]] = None,
    ) -> None:
        if self._ws is None:
            raise RuntimeError("Aelio SDK is not connected yet; call listen() first")
        payload: Dict[str, Any] = {
            "type": "set_flow_progress",
            "customerId": customer_id,
            "flowId": flow_id,
            "stepIndex": step_index,
        }
        if completed_steps is not None:
            payload["completedSteps"] = completed_steps
        await self._ws.send(json.dumps(payload))

    def expose(self, name: str, description: str, params: Dict[str, Any], safety: str, intent: Optional[str] = None):
        def decorator(func: Handler):
            self.handlers[name] = (func, FunctionSchema(description=description, params=params, safety=safety, intent=intent))
            return func

        return decorator

    def persona(self, text: str) -> None:
        """Set the assistant persona/voice; becomes the stable head of the system prompt."""
        self._persona = text.strip()

    def on_send(self, handler: SendHandler) -> None:
        """Deliver outbound messages through your own provider (bring-your-own
        channel). Aelio calls this after each turn; it is not an LLM tool."""
        self._send_handler = handler

    async def ingest(
        self,
        channel: str,
        from_: str,
        text: str,
        message_id: Optional[str] = None,
        metadata: Optional[Dict[str, Any]] = None,
    ) -> None:
        """Hand Aelio an inbound message received on your own channel webhook."""
        if self._ws is None:
            raise RuntimeError("Aelio SDK is not connected yet; call listen() first")
        payload: Dict[str, Any] = {"type": "ingest", "channel": channel, "from": from_, "text": text}
        if message_id is not None:
            payload["messageId"] = message_id
        if metadata is not None:
            payload["metadata"] = metadata
        await self._ws.send(json.dumps(payload))

    async def _send_register(self, websocket) -> None:
        functions = [
            {
                "name": name,
                "description": schema.description,
                "params": schema.params,
                "safety": schema.safety,
                **({"intent": schema.intent} if schema.intent else {}),
            }
            for name, (_, schema) in self.handlers.items()
        ]
        states = [
            {
                "id": state_id,
                "description": schema.description,
                **({"allowedTools": schema.allowed_tools} if schema.allowed_tools else {}),
                **({"blockedTools": schema.blocked_tools} if schema.blocked_tools else {}),
            }
            for state_id, schema in self.states.items()
        ]
        policies = [
            {
                "id": policy_id,
                "description": schema.description,
                "severity": schema.severity,
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
            payload = {
                "type": "result",
                "id": message["id"],
                "ok": True,
                "data": data,
                "durationMs": int(time.time() * 1000) - started,
            }
        except Exception as exc:
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
                        "error": {"code": "NO_SEND_HANDLER", "message": "No on_send handler is registered"},
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
        except Exception as exc:
            await websocket.send(
                json.dumps(
                    {
                        "type": "result",
                        "id": message["id"],
                        "ok": False,
                        "error": {"code": "SEND_FAILED", "message": str(exc), "retryable": True},
                        "durationMs": int(time.time() * 1000) - started,
                    }
                )
            )

    async def listen(self) -> None:
        # Secret travels as an Authorization header, never in the URL.
        endpoint = f"{self.url}{DEFAULT_SDK_PATH}"
        backoff = 1
        while True:
            try:
                async with websockets.connect(
                    endpoint,
                    ping_interval=None,
                    additional_headers={"Authorization": f"Bearer {self.secret}"},
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
                                await websocket.send(json.dumps({"type": "pong", "ts": message.get("ts")}))
                            elif msg_type == "invoke":
                                await self._handle_invoke(websocket, message)
                            elif msg_type == "send":
                                await self._handle_send(websocket, message)
                    finally:
                        heartbeat_task.cancel()
                        self._ws = None
                backoff = 1
            except Exception:
                self._ws = None
                await asyncio.sleep(backoff)
                backoff = min(30, backoff * 2)

    def run(self) -> None:
        asyncio.run(self.listen())
