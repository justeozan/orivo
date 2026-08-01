//! The narrow Wasmtime boundary used by Orivo's plugin registry.
//!
//! This is intentionally only a component preflight for the first host slice.
//! It proves that a verified package is a WebAssembly *component* before an
//! emulator flow may offer it for configuration. Invocation, host functions,
//! grants and `Store` limits are added together in the runner host slice so a
//! partial implementation never turns an opaque launch target into a process.

use wasmtime::{Config, Engine, OptLevel, component::Component};

pub const PLUGIN_WASM_STACK_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug)]
pub struct PluginRuntime {
    engine: Engine,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginRuntimeError {
    EngineUnavailable,
    InvalidComponent,
}

impl std::fmt::Display for PluginRuntimeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EngineUnavailable => {
                write!(formatter, "The Orivo plugin runtime is unavailable.")
            }
            Self::InvalidComponent => write!(
                formatter,
                "The plugin component did not pass WebAssembly validation."
            ),
        }
    }
}

impl std::error::Error for PluginRuntimeError {}

impl PluginRuntime {
    /// Runtime creation happens in a worker after the user opens the extension
    /// surface. It is not part of app startup or rail navigation.
    pub fn new() -> Result<Self, PluginRuntimeError> {
        let mut config = Config::new();
        config
            .wasm_component_model(true)
            .consume_fuel(true)
            .epoch_interruption(true)
            .max_wasm_stack(PLUGIN_WASM_STACK_BYTES)
            .cranelift_opt_level(OptLevel::SpeedAndSize);
        let engine = Engine::new(&config).map_err(|_| PluginRuntimeError::EngineUnavailable)?;
        Ok(Self { engine })
    }

    /// Compilation validates every nested core module in a component. No
    /// guest code runs during this preflight, so fuel is reserved for the
    /// future invocation host but cannot be consumed here.
    pub fn preflight_component(&self, bytes: &[u8]) -> Result<(), PluginRuntimeError> {
        Component::new(&self.engine, bytes)
            .map(|_| ())
            .map_err(|_| PluginRuntimeError::InvalidComponent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Minimal empty Component Model binary: wasm magic, component version,
    // and an empty payload. It is enough to exercise Wasmtime's component
    // validator without executing untrusted guest code.
    const EMPTY_COMPONENT: &[u8] = &[0x00, 0x61, 0x73, 0x6d, 0x0d, 0x00, 0x01, 0x00];

    #[test]
    fn accepts_a_component_and_rejects_non_wasm_bytes() {
        let runtime = PluginRuntime::new().unwrap();
        assert!(runtime.preflight_component(EMPTY_COMPONENT).is_ok());
        assert_eq!(
            runtime.preflight_component(b"not a component").unwrap_err(),
            PluginRuntimeError::InvalidComponent
        );
    }

    #[test]
    fn sdk_wit_contract_is_parseable() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../wit");
        wit_parser::Resolve::default().push_dir(path).unwrap();
    }
}
