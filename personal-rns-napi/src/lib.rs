use napi_derive::napi;

#[napi]
pub fn version() -> String {
    personal_rns_ffi::version()
}

#[napi]
pub struct ReticulumRuntime {
    inner: personal_rns_ffi::ReticulumRuntime,
}

#[napi]
impl ReticulumRuntime {
    #[napi(constructor)]
    pub fn new() -> Self {
        Self {
            inner: personal_rns_ffi::ReticulumRuntime::new(),
        }
    }

    #[napi]
    pub fn tick(&self) -> u64 {
        self.inner.tick()
    }

    #[napi(js_name = "tickCount")]
    pub fn tick_count(&self) -> u64 {
        self.inner.tick_count()
    }
}

impl Default for ReticulumRuntime {
    fn default() -> Self {
        Self::new()
    }
}
