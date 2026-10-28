// Checks the window's frame handling with mock decoders: every decoded frame is closed (counted as open until then), decoder
// errors and unmounts close what they should, and a tile falls back to JPEG after too many errors or after a keyframe with no
// frame drawn within 3 s. Run with: node tests/video-decoders.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { EncodedTiles, ERROR_LIMIT, FIRST_FRAME_MS } from '../src/lib/video-decoders.ts';

// A packet as the engine sends it: version 1, flags (1 keyframe, 2 screen, 4 AV1), frame id, root id, bitstream.
function packet({ root = 'alice', keyframe = false, screen = false, av1 = false, frameId = 1 }) {
  const rootBytes = new TextEncoder().encode(root);
  const bytes = new Uint8Array(8 + rootBytes.length + 4);
  const view = new DataView(bytes.buffer);
  view.setUint8(0, 1);
  view.setUint8(1, (keyframe ? 1 : 0) | (screen ? 2 : 0) | (av1 ? 4 : 0));
  view.setUint32(2, frameId, true);
  view.setUint16(6, rootBytes.length, true);
  bytes.set(rootBytes, 8);
  bytes.set([1, 2, 3, 4], 8 + rootBytes.length);
  return bytes.buffer;
}

// The fakes: a decoder whose outputs the test controls, frames that count as open until closed, and a canvas.
function harness({ outputs = true, throwOnDraw = false } = {}) {
  const state = { open: 0, created: [], warnings: [], keyframeRequests: 0, switched: [], now: 0 };
  const deps = {
    makeDecoder(init) {
      const decoder = {
        state: 'unconfigured',
        output: init.output,
        error: init.error,
        decodes: 0,
        configure() { this.state = 'configured'; },
        decode() {
          this.decodes++;
          if (outputs) this.output(frame());
        },
        close() { this.state = 'closed'; },
      };
      state.created.push(decoder);
      return decoder;
    },
    makeChunk: (init) => init,
    now: () => state.now,
    requestKeyframes: () => { state.keyframeRequests++; },
    switchedToJpeg: (root, reason) => { state.switched.push({ root, reason }); },
  };
  const frame = () => {
    state.open++;
    let closed = false;
    return {
      displayWidth: 1280,
      displayHeight: 720,
      close() {
        if (closed) throw new Error('a frame was closed twice');
        closed = true;
        state.open--;
      },
    };
  };
  const canvas = {
    width: 0,
    height: 0,
    hidden: true,
    getContext() {
      return {
        drawImage() {
          if (throwOnDraw) throw new Error('draw failed');
        },
      };
    },
  };
  state.frame = frame;
  return { state, tiles: new EncodedTiles(deps), canvas };
}

test('every decoded frame is closed once it is drawn', () => {
  const { state, tiles, canvas } = harness();
  tiles.register('alice', canvas);
  tiles.receive(packet({ keyframe: true }));
  for (let i = 2; i <= 30; i++) tiles.receive(packet({ frameId: i }));
  assert.equal(state.open, 0, 'no frame is left open');
  assert.equal(canvas.hidden, false, 'the canvas shows once a frame is drawn');
});

test('a draw that throws still closes the frame', () => {
  const { state, tiles, canvas } = harness({ throwOnDraw: true });
  tiles.register('alice', canvas);
  tiles.receive(packet({ keyframe: true }));
  const decoder = state.created[0];
  assert.throws(() => decoder.output(state.frame()), /draw failed/);
  tiles.receive(packet({ frameId: 2 }));
  assert.equal(state.open, 0, 'a frame whose draw threw is closed');
});

test('an unmounted tile closes its decoder, and a late frame from it is still closed', () => {
  const { state, tiles, canvas } = harness();
  tiles.register('alice', canvas);
  tiles.receive(packet({ keyframe: true }));
  const decoder = state.created[0];
  tiles.register('alice', null);
  assert.equal(decoder.state, 'closed', 'the decoder is closed with the tile');
  decoder.output(state.frame());
  assert.equal(state.open, 0, 'a frame that arrives after the tile is gone is closed');
});

test('a decoder error starts a fresh decoder on the next keyframe, and asks for a keyframe', () => {
  const { state, tiles, canvas } = harness();
  tiles.register('alice', canvas);
  tiles.receive(packet({ keyframe: true }));
  state.created[0].error(new Error('corrupt'));
  assert.equal(state.keyframeRequests, 1, 'the window asks for a keyframe after an error');
  assert.equal(state.created[0].state, 'configured', 'the failed decoder is not reused');
  tiles.receive(packet({ frameId: 2 }));
  assert.equal(state.keyframeRequests, 2, 'a delta with no decoder asks again');
  tiles.receive(packet({ keyframe: true, frameId: 3 }));
  assert.equal(state.created.length, 2, 'a keyframe starts a fresh decoder');
  assert.equal(state.open, 0);
  assert.deepEqual(state.switched, [], 'one error does not switch the tile');
});

test('more than the error limit within 10 s switches the tile to JPEG, once', () => {
  const { state, tiles, canvas } = harness();
  tiles.register('alice', canvas);
  for (let i = 0; i <= ERROR_LIMIT; i++) {
    state.now = i * 1000;
    tiles.receive(packet({ keyframe: true, frameId: i + 1 }));
    state.created.at(-1).error(new Error('corrupt'));
  }
  assert.equal(state.switched.length, 1, 'the tile switches once');
  assert.equal(state.switched[0].root, 'alice');
  assert.match(state.switched[0].reason, /failed \d+ times in 10 s/);
  assert.ok(tiles.fallbacks().has('alice'));
  const before = state.created.length;
  tiles.receive(packet({ keyframe: true, frameId: 99 }));
  assert.equal(state.created.length, before, 'frames from a switched tile are not decoded here');
});

test('errors spread further apart than 10 s do not switch the tile', () => {
  const { state, tiles, canvas } = harness();
  tiles.register('alice', canvas);
  for (let i = 0; i < ERROR_LIMIT + 2; i++) {
    state.now = i * 11_000;
    tiles.receive(packet({ keyframe: true, frameId: i + 1 }));
    state.created.at(-1).error(new Error('corrupt'));
  }
  assert.deepEqual(state.switched, []);
});

test('no frame drawn within 3 s of a keyframe switches the tile to JPEG', () => {
  const { state, tiles, canvas } = harness({ outputs: false });
  tiles.register('alice', canvas);
  state.now = 0;
  tiles.receive(packet({ keyframe: true }));
  state.now = FIRST_FRAME_MS;
  tiles.receive(packet({ frameId: 2 }));
  assert.deepEqual(state.switched, [], 'exactly 3 s is not yet late');
  state.now = FIRST_FRAME_MS + 1;
  tiles.receive(packet({ frameId: 3 }));
  assert.equal(state.switched.length, 1);
  assert.match(state.switched[0].reason, /within 3 s of a keyframe/);
});

test('a frame drawn after the keyframe clears the deadline', () => {
  const { state, tiles, canvas } = harness();
  tiles.register('alice', canvas);
  state.now = 0;
  tiles.receive(packet({ keyframe: true }));
  state.now = 10_000;
  tiles.receive(packet({ keyframe: true, frameId: 2 }));
  for (let i = 3; i < 10; i++) {
    state.now += 100;
    tiles.receive(packet({ frameId: i }));
  }
  assert.deepEqual(state.switched, [], 'a keyframe whose frames draw is not late');
});

test('a screen and a camera from one peer fall back together, and other peers are unaffected', () => {
  const { state, tiles, canvas } = harness();
  tiles.register('alice', canvas);
  tiles.register('alice:screen', canvas);
  tiles.register('bob', canvas);
  for (let i = 0; i <= ERROR_LIMIT; i++) {
    state.now = i * 500;
    tiles.receive(packet({ root: 'alice', keyframe: true, frameId: i + 1 }));
    state.created.at(-1).error(new Error('corrupt'));
  }
  assert.ok(tiles.fallbacks().has('alice'));
  assert.ok(!tiles.fallbacks().has('bob'));
  state.now = 5000;
  tiles.receive(packet({ root: 'bob', keyframe: true, frameId: 1 }));
  assert.equal(state.created.at(-1).state, 'configured', 'bob still decodes');
});

test('clearing the fallbacks starts the next call on the encoded path', () => {
  const { state, tiles, canvas } = harness();
  tiles.register('alice', canvas);
  for (let i = 0; i <= ERROR_LIMIT; i++) {
    state.now = i * 100;
    tiles.receive(packet({ keyframe: true, frameId: i + 1 }));
    state.created.at(-1).error(new Error('corrupt'));
  }
  tiles.clearFallbacks();
  assert.equal(tiles.fallbacks().size, 0);
  tiles.receive(packet({ keyframe: true, frameId: 50 }));
  assert.equal(state.created.at(-1).state, 'configured');
});
