use anyhow::Result;
use std::path::Path;

pub struct WasmPlugin {
    name: String,
    engine: wasmtime::Engine,
    module: wasmtime::Module,
    hooks: Vec<String>,
}

impl WasmPlugin {
    pub fn load(path: &Path, name: &str) -> Result<Self> {
        let wasm_bytes = std::fs::read(path)?;
        let config = wasmtime::Config::new();
        let engine = wasmtime::Engine::new(&config)?;
        let module = wasmtime::Module::new(&engine, &wasm_bytes)?;
        Ok(Self {
            name: name.to_string(),
            engine,
            module,
            hooks: Vec::new(),
        })
    }

    pub fn instantiate(&mut self, hooks: Vec<String>) -> Result<()> {
        self.hooks = hooks;
        tracing::info!(plugin = %self.name, hooks = ?self.hooks, "WASM plugin ready");
        Ok(())
    }

    pub fn call_hook(&self, hook_name: &str, payload: &[u8]) -> Result<Vec<u8>> {
        if !self.hooks.contains(&hook_name.to_string()) {
            return Ok(payload.to_vec());
        }

        let mut store = wasmtime::Store::new(&self.engine, PluginCtx::new(&self.name));
        let instance = wasmtime::Instance::new(&mut store, &self.module, &[])?;

        let func = match instance.get_func(&mut store, hook_name) {
            Some(f) => f,
            None => return Ok(payload.to_vec()),
        };

        let payload_str = String::from_utf8_lossy(payload).to_string();
        let payload_bytes = payload_str.as_bytes().to_vec();

        let memory = match instance.get_memory(&mut store, "memory") {
            Some(m) => m,
            None => {
                let params = [wasmtime::Val::I32(0), wasmtime::Val::I32(0)];
                let mut results = [wasmtime::Val::I32(0), wasmtime::Val::I32(0)];
                let _ = func.call(&mut store, &params, &mut results);
                return Ok(payload.to_vec());
            }
        };

        let alloc_ptr = instance
            .get_global(&mut store, "__alloc")
            .and_then(|g| match g.get(&mut store) {
                wasmtime::Val::I32(p) if p > 0 => Some(p as usize),
                _ => None,
            })
            .unwrap_or(memory.data(&store).len());

        let ptr = alloc_ptr;
        if ptr + payload_bytes.len() <= memory.data(&store).len() {
            memory.data_mut(&mut store)[ptr..ptr + payload_bytes.len()]
                .copy_from_slice(&payload_bytes);
        }

        let params = [
            wasmtime::Val::I32(ptr as i32),
            wasmtime::Val::I32(payload_bytes.len() as i32),
        ];
        let mut results = [wasmtime::Val::I32(0), wasmtime::Val::I32(0)];

        match func.call(&mut store, &params, &mut results) {
            Ok(_) => {
                let out_ptr = results[0].i32().unwrap_or(0) as usize;
                let out_len = results[1].i32().unwrap_or(0) as usize;

                if out_ptr > 0 && out_len > 0 && out_ptr + out_len <= memory.data(&store).len() {
                    let output = memory.data(&store)[out_ptr..out_ptr + out_len].to_vec();
                    Ok(output)
                } else {
                    Ok(payload.to_vec())
                }
            }
            Err(e) => {
                tracing::debug!(plugin = %self.name, hook = %hook_name, error = %e, "Plugin hook call failed");
                Ok(payload.to_vec())
            }
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

struct PluginCtx {
    _name: String,
}

impl PluginCtx {
    fn new(name: &str) -> Self {
        Self {
            _name: name.to_string(),
        }
    }
}

pub struct PluginManager {
    plugins: Vec<WasmPlugin>,
    plugin_dirs: Vec<std::path::PathBuf>,
}

impl PluginManager {
    pub fn new(plugin_dirs: Vec<std::path::PathBuf>) -> Self {
        Self {
            plugins: Vec::new(),
            plugin_dirs,
        }
    }

    pub fn discover_and_load(&mut self) -> Result<()> {
        for dir in &self.plugin_dirs {
            if !dir.exists() {
                std::fs::create_dir_all(dir)?;
                continue;
            }

            for entry in std::fs::read_dir(dir)? {
                let entry = entry?;
                let path = entry.path();
                if path.extension().map(|e| e == "wasm").unwrap_or(false) {
                    let name = path.file_stem().unwrap().to_string_lossy().to_string();
                    match WasmPlugin::load(&path, &name) {
                        Ok(mut plugin) => {
                            if plugin
                                .instantiate(vec![
                                    "on_frame_captured".to_string(),
                                    "on_frame_received".to_string(),
                                    "on_input_event".to_string(),
                                ])
                                .is_ok()
                            {
                                tracing::info!(plugin = %name, "Plugin loaded");
                                self.plugins.push(plugin);
                            }
                        }
                        Err(e) => {
                            tracing::warn!(plugin = %name, error = %e, "Failed to load plugin");
                        }
                    }
                }
            }
        }
        Ok(())
    }

    pub fn call_hook_all(&self, hook_name: &str, payload: &[u8]) -> Result<Vec<u8>> {
        let mut current = payload.to_vec();
        for plugin in &self.plugins {
            match plugin.call_hook(hook_name, &current) {
                Ok(result) => {
                    if !result.is_empty() {
                        current = result;
                    }
                }
                Err(e) => {
                    tracing::warn!(plugin = %plugin.name(), hook = %hook_name, error = %e, "Hook failed");
                }
            }
        }
        Ok(current)
    }
}

impl Default for PluginManager {
    fn default() -> Self {
        Self::new(vec![
            std::path::PathBuf::from("plugins"),
            dirs::data_dir()
                .map(|p| p.join("continuum").join("plugins"))
                .unwrap_or_else(|| std::path::PathBuf::from(".continuum/plugins")),
        ])
    }
}
