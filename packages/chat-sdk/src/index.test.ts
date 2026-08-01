import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import {
  flattenToText,
  makeHello,
  textFrame,
  validateEventFrame,
  validateRenderFrame,
  messageEvent,
  CORE_KINDS,
} from './index.js';

describe('chat-sdk render protocol', () => {
  it('validates text frames', () => {
    const f = textFrame('t1', 'f1', 'Hello');
    const r = validateRenderFrame(f);
    assert.equal(r.ok, true);
    assert.equal(flattenToText(f), 'Hello');
  });

  it('rejects choice without fallback', () => {
    const r = validateRenderFrame({
      frame: 'render',
      frame_id: 'f1',
      mode: 'append',
      turn_id: 't1',
      blocks: [
        {
          block_id: 'c1',
          kind: 'choice@1',
          body: { options: [{ id: 'a', label: 'A' }], multi: false },
        },
      ],
    });
    assert.equal(r.ok, false);
  });

  it('hello includes core kinds', () => {
    const h = makeHello();
    assert.ok(h.kinds.includes('text@1'));
    assert.equal(h.kinds.length, CORE_KINDS.length);
  });

  it('validates message events', () => {
    const e = messageEvent('t1', 'e1', 'hi');
    const r = validateEventFrame(e);
    assert.equal(r.ok, true);
  });
});
