// Focused actual-WASM lifecycle checks, without a browser/GPU or any build step.
// node --test renderer/web/tests/session.mjs
// THREERT_WEB_PKG may point to an already built pkg directory.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { test } from 'node:test';

const pkg = resolve(process.env.THREERT_WEB_PKG ?? 'out/web/dist/pkg');
const wasm = await import(pathToFileURL(resolve(pkg, 'threert_web.js')).href);
wasm.initSync({ module: readFileSync(resolve(pkg, 'threert_web_bg.wasm')) });
const state = () => JSON.parse(wasm.sessionState());
// No canvas methods/GPU are touched when same-task cancellation works correctly.
const canvas = { width: 160, height: 90 };
const start = () => wasm.start(canvas, 160, 90, '{}', 1, 60, 8192);
const cancelled = (promise) => assert.rejects(promise, (e) => e.name === 'AbortError');

test('dispose cancels a start before its first async poll', async () => {
  wasm.reset();
  const p = start();
  assert.equal(state().pending, true);
  wasm.dispose();
  await cancelled(p);
  assert.equal(state().pending, false);
  assert.equal(state().hasView, false);
});

test('reset cancels a same-task start and clears supplied bytes', async () => {
  wasm.reset();
  wasm.provide('fixture.bin', new Uint8Array([1, 2, 3, 4]));
  assert.equal(state().resources.providedBytes, 4);
  const p = start();
  wasm.reset();
  await cancelled(p);
  assert.equal(state().resources.providedFiles, 0);
  assert.equal(state().resources.providedBytes, 0);
  assert.equal(state().hasView, false);
});

test('superseded starts cannot revive the session or finish a newer ticket', async () => {
  wasm.reset();
  const first = start();
  const second = start();
  wasm.dispose();
  await Promise.all([cancelled(first), cancelled(second)]);
  assert.equal(state().pending, false);
  assert.equal(state().hasView, false);
});

test('invalid output sizes reject before replacing the pending session', async () => {
  wasm.reset();
  const p = start();
  const before = state().generation;
  const bad = wasm.start(canvas, 7, 90, '{}', 1, 60, 8192);
  assert.equal(state().generation, before);
  assert.equal(state().pending, true);
  wasm.dispose();
  await assert.rejects(bad, /at least 8/);
  await cancelled(p);
});

test('dispose preserves world uploads; reset clears them and permits the same IDs again', () => {
  wasm.reset();
  const geometry = () => wasm.geometry(7,
    new Float32Array([0, 0, 0, 1, 0, 0, 0, 1, 0]),
    new Float32Array([0, 0, 1, 0, 0, 1, 0, 0, 1]),
    new Float32Array([0, 0, 1, 0, 0, 1]), new Uint32Array([0, 1, 2]));
  geometry();
  wasm.provide('fixture.bin', new Uint8Array([1, 2, 3, 4]));
  wasm.dispose();
  assert.equal(state().resources.geometries, 1);
  assert.equal(state().resources.providedBytes, 4);
  wasm.reset();
  assert.equal(state().resources.geometries, 0);
  assert.equal(state().resources.providedBytes, 0);
  geometry();
  assert.equal(state().resources.geometries, 1);
  wasm.reset();
});

test('frame and resize after dispose are explicit errors, not use-after-free', async () => {
  wasm.dispose();
  assert.throws(() => wasm.frameBegin(1 / 60), /no view/);
  assert.throws(() => wasm.frameEnd(1 / 60), /no view/);
  assert.throws(() => wasm.resizeSurface(320, 180), /no view/);
  await assert.rejects(wasm.waitForFrame(), /no view/);
  assert.equal(state().pendingFrameWaits, 0);
  wasm.dispose();
  wasm.reset();
  assert.equal(state().pending, false);
});

test('mobile reservations are explicit, preserve uploads, and reset restores desktop', () => {
  wasm.reset();
  assert.deepEqual(state().resourceProfile, { name: 'desktop', cacheEntries: 262144, editReserveScale: 1, maxOutputPixels: null });
  wasm.provide('profile-fixture.bin', new Uint8Array([3, 1, 4]));
  const before = state().resources;
  const mobile = JSON.parse(wasm.configureResourceProfile('mobile'));
  assert.deepEqual(mobile, { name: 'mobile', cacheEntries: 65536, editReserveScale: 0, maxOutputPixels: 65536 });
  assert.deepEqual(state().resources, before, 'choosing reservations must not remove scene resources');
  assert.throws(() => wasm.configureResourceProfile('Mobile'), /desktop.*mobile/);
  assert.deepEqual(state().resourceProfile, mobile);
  wasm.dispose();
  assert.deepEqual(state().resourceProfile, mobile, 'view-only disposal preserves the chosen profile');
  wasm.configureResourceProfile('desktop');
  assert.equal(state().resourceProfile.cacheEntries, 262144);
  wasm.configureResourceProfile('mobile');
  wasm.reset();
  assert.equal(state().resourceProfile.name, 'desktop');
  assert.equal(state().resources.providedBytes, 0);
});

test('pending start locks its resource profile until cancelled', async () => {
  wasm.reset();
  wasm.configureResourceProfile('mobile');
  const p = start();
  assert.throws(() => wasm.configureResourceProfile('desktop'), /dispose/);
  assert.equal(state().pending, true);
  assert.equal(state().resourceProfile.name, 'mobile');
  wasm.dispose();
  wasm.configureResourceProfile('desktop');
  await cancelled(p);
  assert.equal(state().resourceProfile.name, 'desktop');
});

test('mobile output cap rejects before cancelling a valid pending start', async () => {
  wasm.reset();
  wasm.configureResourceProfile('mobile');
  const p = start(), before = state().generation;
  const bad = wasm.start(canvas, 640, 360, '{}', 1, 30, 65536);
  assert.equal(state().generation, before);
  assert.equal(state().pending, true);
  wasm.reset();
  await assert.rejects(bad, /65536 pixels/);
  await cancelled(p);
  assert.equal(state().resourceProfile.name, 'desktop');
});
