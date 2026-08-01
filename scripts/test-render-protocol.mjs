import assert from 'node:assert/strict';
import {
  flattenToText,
  makeHello,
  intersectKinds,
  CORE_KINDS,
  validateRenderFrame,
  validateEventFrame,
  messageEvent,
  textFrame,
} from '../packages/chat-sdk/dist/index.js';

// Simulate Rust JSON (serde rename frame_type -> frame)
const rustLike = {
  frame: 'render',
  frame_id: 'rf-3',
  mode: 'append',
  turn_id: 'ingress:abc',
  blocks: [{ block_id: 'b0', kind: 'text@1', body: { md: 'Your order **shipped**.' } }],
};

const hello = makeHello();
const accepted = intersectKinds(hello.kinds, CORE_KINDS);
assert.ok(accepted.includes('text@1'));

const checked = validateRenderFrame(rustLike, accepted);
assert.equal(checked.ok, true);
assert.match(flattenToText(checked.frame), /shipped/);

const confirmLike = {
  frame: 'render',
  frame_id: 'rf-4',
  mode: 'append',
  turn_id: 't2',
  blocks: [
    {
      block_id: 'c0',
      kind: 'confirm@1',
      body: { prompt: 'Please confirm cancel', yes_label: 'Confirm', no_label: 'Cancel', danger: false },
      fallback: { block_id: 'c0_fb', kind: 'text@1', body: { md: 'Please confirm cancel (yes/no)' } },
    },
  ],
};
assert.equal(validateRenderFrame(confirmLike, accepted).ok, true);

const ev = messageEvent('t1', 'e1', 'hello');
assert.equal(validateEventFrame(ev).ok, true);

const fallback = textFrame('t', 'f', 'ok');
assert.equal(validateRenderFrame(fallback).ok, true);

console.log('[Render Protocol] PASSED — Hello/Welcome kinds, text@1 + confirm@1 frames, EventFrame message');
