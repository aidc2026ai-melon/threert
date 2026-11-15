//! Browser GPU errors are asynchronous. Queue completion alone is not proof that a frame was rendered.
//! Use raw WebGPU promises/error objects: wgpu 30 calls completion callbacks even on rejected fences and
//! its error conversion does not handle GPUInternalError. Its event-handler property also misses older WebKit.

use js_sys::{Array, Function, Promise, Reflect, Uint32Array};
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use wasm_bindgen::{closure::Closure, JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use super::frame_probe;

pub(super) type ScopeCheck = (&'static str, Promise);

fn call(object: &JsValue, name: &str, args: &[JsValue]) -> Result<JsValue, JsValue> {
    let f: Function = Reflect::get(object, &name.into())?.dyn_into()?;
    let list = Array::new();
    for arg in args { list.push(arg); }
    f.apply(object, &list)
}

fn text(object: &JsValue, key: &str) -> String {
    Reflect::get(object, &key.into()).ok().and_then(|s| s.as_string()).unwrap_or_default()
}

#[derive(Default)]
struct Status {
    active: bool,
    stage: String,
    error: Option<(String, String)>,
    first_frame_verified: bool,
    probe: Option<(usize, usize, [u8; 3], [u8; 3])>,
}

fn record(state: &Weak<RefCell<Status>>, kind: &str, message: &str) {
    if let Some(state) = state.upgrade() {
        let mut s = state.borrow_mut();
        if s.active && s.error.is_none() {
            s.error = Some((kind.to_owned(), format!("{}: {}", s.stage, message.chars().take(400).collect::<String>())));
        }
    }
}

struct Inner {
    raw: JsValue,
    state: Rc<RefCell<Status>>,
    listener: RefCell<Option<Closure<dyn FnMut(JsValue)>>>,
    scope_depth: Cell<usize>,
    probe: RefCell<Option<wgpu::Buffer>>,
}

impl Inner {
    fn close(&self) {
        let was_active = std::mem::replace(&mut self.state.borrow_mut().active, false);
        if !was_active { return; }
        if let Some(listener) = self.listener.borrow_mut().take() {
            let _ = call(&self.raw, "removeEventListener", &["uncapturederror".into(), listener.as_ref().clone()]);
        }
        if let Some(buffer) = self.probe.borrow_mut().take() { buffer.destroy(); }
        while self.scope_depth.get() > 0 {
            self.scope_depth.set(self.scope_depth.get() - 1);
            let _ = call(&self.raw, "popErrorScope", &[]);
        }
        // ACTIVE_HEALTH also owns a device while start() awaits error scopes, before View exists.
        // Destroy it immediately on reset, rather than waiting for that startup future to settle.
        let _ = call(&self.raw, "destroy", &[]);
    }
}

impl Drop for Inner { fn drop(&mut self) { self.close(); } }

#[derive(Clone)]
pub(super) struct GpuHealth(Rc<Inner>);

struct ProbeLease { health: GpuHealth, buffer: wgpu::Buffer }
impl Drop for ProbeLease {
    fn drop(&mut self) {
        self.buffer.destroy();
        self.health.0.probe.borrow_mut().take();
    }
}

impl GpuHealth {
    pub fn new(device: &wgpu::Device) -> Result<Self, JsValue> {
        #[cfg(target_arch = "wasm32")]
        let raw = <_ as AsRef<JsValue>>::as_ref(device.as_webgpu().ok_or_else(|| js_sys::Error::new("WebGPU device unavailable"))?).clone();
        #[cfg(not(target_arch = "wasm32"))]
        let raw: JsValue = { let _ = device; return Err(js_sys::Error::new("Browser GPU health requires WebGPU").into()); };
        let state = Rc::new(RefCell::new(Status { active: true, stage: "device-setup".into(), ..Default::default() }));
        let weak = Rc::downgrade(&state);
        let listener = Closure::wrap(Box::new(move |event: JsValue| {
            let error = Reflect::get(&event, &"error".into()).unwrap_or(event);
            let name = text(&error, "name");
            record(&weak, if name.is_empty() { "GPU_ERROR" } else { &name }, &text(&error, "message"));
        }) as Box<dyn FnMut(JsValue)>);
        call(&raw, "addEventListener", &["uncapturederror".into(), listener.as_ref().clone()])?;
        let health = Self(Rc::new(Inner { raw, state, listener: RefCell::new(Some(listener)), scope_depth: Cell::new(0), probe: RefCell::new(None) }));
        let weak = Rc::downgrade(&health.0.state);
        let lost = Reflect::get(&health.0.raw, &"lost".into())?;
        // The once closure owns only a Weak status reference. Disposal closes status before device.destroy(),
        // so the normal destroyed event and an old session's late event cannot poison a replacement session.
        let callback = Closure::once_into_js(move |info: JsValue| {
            record(&weak, "GPU_DEVICE_LOST", &format!("{} {}", text(&info, "reason"), text(&info, "message")));
        });
        call(&lost, "then", &[callback])?;
        Ok(health)
    }

    pub fn close(&self) { self.0.close(); }

    pub fn check(&self) -> Result<(), JsValue> {
        let s = self.0.state.borrow();
        if !s.active { return Err(js_sys::Error::new("GPU session disposed").into()); }
        if let Some((kind, message)) = &s.error { return Err(js_sys::Error::new(&format!("{kind}: {message}")).into()); }
        Ok(())
    }

    fn error(&self, kind: &str, error: &JsValue) -> JsValue {
        let message = text(error, "message");
        let message = if message.is_empty() { error.as_string().unwrap_or_else(|| format!("{error:?}")) } else { message };
        record(&Rc::downgrade(&self.0.state), kind, &message);
        js_sys::Error::new(&format!("{kind}: {message}")).into()
    }

    pub fn snapshot(&self) -> Value {
        let s = self.0.state.borrow();
        let mut out = json!({ "status": if s.error.is_some() { "error" } else { "ok" },
            "stage": if s.active { s.stage.as_str() } else { "disposed" }, "firstFrameVerified": s.first_frame_verified });
        if let Some((kind, message)) = &s.error { out["errorType"] = json!(kind); out["message"] = json!(message); }
        if let Some((sampled, nonzero, min, max)) = s.probe { out["probe"] = json!({"sampledPixels": sampled, "nonzeroRgbSamples": nonzero, "rgbMin": min, "rgbMax": max}); }
        out
    }

    pub fn begin_scopes(&self, stage: &str) -> Result<(), JsValue> {
        self.check()?;
        if self.0.scope_depth.get() != 0 { return Err(js_sys::Error::new("GPU error scopes already open").into()); }
        self.0.state.borrow_mut().stage = stage.into();
        for filter in ["out-of-memory", "internal", "validation"] {
            if let Err(e) = call(&self.0.raw, "pushErrorScope", &[filter.into()]) {
                let _ = self.end_scopes();
                return Err(self.error("GPU_ERROR_SCOPE", &e));
            }
            self.0.scope_depth.set(self.0.scope_depth.get() + 1);
        }
        Ok(())
    }

    pub fn end_scopes(&self) -> Result<Vec<ScopeCheck>, JsValue> {
        let mut checks = Vec::new();
        let mut failure = None;
        while self.0.scope_depth.get() > 0 {
            self.0.scope_depth.set(self.0.scope_depth.get() - 1);
            match call(&self.0.raw, "popErrorScope", &[]).and_then(|p| p.dyn_into::<Promise>()) {
                Ok(p) => checks.push((["GPU_OUT_OF_MEMORY", "GPU_INTERNAL", "GPU_VALIDATION"][self.0.scope_depth.get()], p)), Err(e) => { failure = Some(e); }
            }
        }
        if let Some(e) = failure { return Err(self.error("GPU_ERROR_SCOPE", &e)); }
        Ok(checks)
    }

    pub async fn check_scopes(&self, checks: Vec<ScopeCheck>) -> Result<(), JsValue> {
        for (kind, promise) in checks {
            match JsFuture::from(promise).await {
                Ok(error) if !error.is_null() && !error.is_undefined() => { self.error(kind, &error); },
                Err(error) => { self.error("GPU_ERROR_SCOPE", &error); },
                _ => {},
            }
        }
        self.check()
    }

    pub async fn wait_submission(&self) -> Result<(), JsValue> {
        self.check()?;
        let queue = Reflect::get(&self.0.raw, &"queue".into())?;
        let promise: Promise = call(&queue, "onSubmittedWorkDone", &[])?.dyn_into()?;
        JsFuture::from(promise).await.map_err(|e| self.error("GPU_SUBMISSION", &e))?;
        self.check()
    }

    /// A dark frame is valid. The post shader writes alpha=255 even for black, whereas an unwritten output is 0.
    /// Nine dispersed output words catch an invalid/no-op first dispatch without copying or retaining the image.
    /// This verifies output writes, not image correctness or presentation; an opaque black miscompile can pass.
    pub async fn verify_first_frame(&self, device: wgpu::Device, queue: wgpu::Queue, output: wgpu::Buffer, width: u32, height: u32) -> Result<(), JsValue> {
        self.check()?;
        if self.0.state.borrow().first_frame_verified { return Ok(()); }
        self.begin_scopes("first-frame-readback")?;
        let read = device.create_buffer(&wgpu::BufferDescriptor { label: Some("first frame probe"), size: 36,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        *self.0.probe.borrow_mut() = Some(read.clone());
        let _lease = ProbeLease { health: self.clone(), buffer: read.clone() };
        let mut encoder = device.create_command_encoder(&Default::default());
        for (n, offset) in frame_probe::offsets(width, height).into_iter().enumerate() {
            encoder.copy_buffer_to_buffer(&output, offset, &read, n as u64 * 4, 4);
        }
        queue.submit([encoder.finish()]);
        let checks = self.end_scopes()?;
        #[cfg(target_arch = "wasm32")]
        let raw = <_ as AsRef<JsValue>>::as_ref(read.as_webgpu().ok_or_else(|| js_sys::Error::new("WebGPU probe unavailable"))?).clone();
        #[cfg(not(target_arch = "wasm32"))]
        let raw: JsValue = { return Err(js_sys::Error::new("WebGPU probe unavailable").into()); };
        let result = async {
            let promise: Promise = call(&raw, "mapAsync", &[JsValue::from_f64(1.0)])?.dyn_into()?;
            JsFuture::from(promise).await.map_err(|e| self.error("GPU_PROBE_MAP", &e))?;
            self.check_scopes(checks).await?;
            let values = Uint32Array::new(&call(&raw, "getMappedRange", &[])?) .to_vec();
            call(&raw, "unmap", &[])?;
            let nonzero = frame_probe::validate(&values).map_err(|e| self.error("GPU_FIRST_FRAME_UNWRITTEN", &e.into()))?;
            self.check()?;
            let mut s = self.0.state.borrow_mut();
            s.first_frame_verified = true;
            let (min, max) = frame_probe::rgb_bounds(&values);
            s.probe = Some((values.len(), nonzero, min, max));
            s.stage = "rendering".into();
            Ok(())
        }.await;
        result
    }
}
