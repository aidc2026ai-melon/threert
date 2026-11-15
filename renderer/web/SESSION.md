# Browser session lifecycle

The page owns its animation loop, input listeners and scene binding. The WASM module owns the world, uploaded files
and the WebGPU view. Stop the page's loop/listeners before tearing down or resetting the module.

| Export | Contract |
| --- | --- |
| `start(canvas, width, height, describeJSON, scale, fps, guard): Promise<string>` | Opens the current world. A valid call synchronously cancels older starts and destroys the previous view before requesting a new GPU device. Its JSON result has adapter limits and load diagnostics. Output width/height must be at least 8. |
| `dispose(): void` | Cancels pending starts and explicitly destroys the current GPU view. Keeps the world, provided files and decoded caches so the same scene can be restarted without geometry retransmission. Repeated calls are safe. |
| `reset(): void` | Disposes, replaces world/physics/voxels, and clears provided files plus decoded model/image/procedural/environment caches. Discard the old JS SceneBinding and upload a fresh scene. Object IDs may be reused in that new world. |
| `resizeSurface(width, height): void` | Changes the canvas backing size and its presentation configuration/pipeline. Keeps the RT output dimensions and storage targets fixed, bilinearly enlarging with aspect-preserving letterbox bars. This does not add rendered detail. |
| `waitForFrame(): Promise<void>` | Waits for queue completion notification for work submitted before the call. Await this after `frameEnd()` before submitting another frame to bound in-flight work, including the initial shader compilation. Teardown rejects pending waits with `AbortError`. |
| `sessionState(): string` | JSON with generation, pending, hasView, output/surface dimensions, frame and content-free world/file counts. |
| `configureResourceProfile('mobile' \| 'desktop'): string` | Selects allocation reservations with no live view or pending start; otherwise throws. Returns JSON with name, cacheEntries, editReserveScale and maxOutputPixels. `dispose()` preserves this choice; `reset()` returns to desktop. |

## Mobile resource reservations

Use `reset(); configureResourceProfile('mobile');` before uploading the scene and calling `start()`.
The mobile profile uses 65,536 radiance-cache entries (4.25 MiB for the three cache arrays, versus desktop's
262,144 entries / 17 MiB), reserves no additional instance/object/triangle space for future edits, and rejects
output above 65,536 pixels. For example, 320 x 180 is accepted; 640 x 360 is rejected without changing an existing
view or pending start. There is no automatic geometry omission, instance reduction or output rescaling.
The full scene must still pass the existing geometry/BVH/adapter binding limits. Existing camera/transform edits
are supported; a structural addition that needs reserved space can require a scene rebuild.

The smaller cache can increase hash collisions, noise or tracing work. Output/history storage, scene geometry,
WASM/CPU copies, shader compilation and the host's other renderer still consume memory. This is not a device-memory
guarantee. Hosts must bound in-flight frames and verify actual target-device behavior independently.
The profile's settings are included in both `sessionState().resourceProfile` and the resolved `start()` JSON.
Native execution remains on desktop defaults, retaining `THREERT_CACHE` and `THREERT_EDIT_RESERVE` behavior.
The new mobile package is built separately; hosts can retain the previous desktop package unchanged.

`start(); dispose()` and `start(); reset()` in the same JavaScript task cancel correctly: the generation is acquired
before the future's first poll. Superseded/cancelled promises reject with an Error whose `name` is `AbortError`.
Cancellation is checked around adapter/device awaits. A device returned after cancellation is destroyed before
its scene buffers are built. Synchronous CPU scene construction or shader compilation cannot be interrupted by a
later JavaScript call until control returns to the browser. Result-returning initialization failures after GPU
acquisition destroy the partial device. An unrecoverable Rust panic still makes a WASM instance unusable.

Explicit teardown normally resolves WebGPU's `device.lost` with `reason === "destroyed"`; record this as intentional
teardown rather than an unexpected device failure. Releasing Rust allocations makes WASM heap memory reusable;
it does not shrink `WebAssembly.Memory` or promise an immediate operating-system memory reduction.

After `dispose()`, `frameBegin`, `frameEnd`, `click`, `commit`, `resizeSurface` and `waitForFrame` reject until a new start resolves.
After `reset()`, existing JS binding caches are invalid: reusing them can skip uploads of now-missing engine handles.

`sessionState().pendingFrameWaits` counts outstanding queue notifications. The host should normally have at most
one: `frameBegin(dt)` / script / `frameEnd(dt)` / `await waitForFrame()`, then schedule the next animation frame.
Wgpu 30's WebGPU backend calls its completion callback on both successful completion and an error rejection of
the underlying queue promise. Therefore `waitForFrame()` supplies backpressure, not proof of correct pixels or
device health. Keep device-loss/uncaptured-error and loop-guard checks. Waiting does not itself submit a frame.
After the await, check the host's own generation/disposed flag again before submitting: teardown can happen after
a completion promise has already resolved, in which case it is too late to reject that settled promise.

## Camera and selection integration

Use the existing world API for camera controls; it survives view-only dispose/restart:

- `camera(id, descriptorJSON)` sets the lens/exposure of an existing camera object.
- `transform(id, Float32Array)` sets its pose with 10 values: position xyz, quaternion xyzw, scale xyz.
- `settings(descriptorJSON)` selects `camera: id` alongside the scene's other settings. Preserve those other values;
  this function supplies defaults for fields that are omitted.
- `commit(seq)` or the next `frameEnd(dt)` applies changes and follows the active scene camera.

`click(u,v)` accepts normalized canvas coordinates, top-left origin, and maps through the same letterbox as the
blit shader. A bar returns no hit. A hit identifies the object's name and optional instanceId; the RHODE adapter
must retain the name/instance-slot to source-object mapping. CPU pick cost on a large model remains a separate
runtime measurement. Camera near/far descriptor fields are not a BIM section/clipping-plane implementation.

## Instance and device limits

Software unit traversal permits up to 524,288 units. Its shared stack remains 64 entries: top-tree depth at most
20, mesh-tree depth at most 44. WASM scene loading refuses a larger instance set instead of silently flattening it.
The count limit is not a guarantee that every such scene fits a device: initial upload validates major geometry
buffers and the actual rounded merged-table binding against that adapter's `maxStorageBufferBindingSize` and
`maxBufferSize`, including edit reserve. Live output buffers receive the same preflight. A rejected startup returns
an error and destroys the partial GPU session. This is a binding-size check, not an available-VRAM estimate.

The `start()` JSON `load` object includes `merged_tables_bytes`, `instance_records_bytes`, and
`object_records_bytes`. Dynamic arena growth beyond the initial upload remains a separate path; these startup
checks do not claim to validate every future resource mutation.

## Focused verification

`renderer/web/src/lifecycle.rs` has pure Rust epoch, size and letterbox mapping tests. With a freshly built web
package, `node --test renderer/web/tests/session.mjs` exercises the actual WASM exports without a browser/GPU.
Set `THREERT_WEB_PKG` to the generated package directory if it is not `out/web/dist/pkg`.

Actual GPU QA must additionally exercise start/render/dispose/restart on the same canvas, cancellation while
adapter/device promises are pending, failed scene construction followed by a good start, presentation resize,
camera move and reset/full reupload. Normal `destroyed` device-loss notifications must be distinguished from
unexpected failures. No test listed here is claimed to have passed merely by being documented.
