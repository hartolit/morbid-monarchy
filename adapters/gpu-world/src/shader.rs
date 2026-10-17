//! WGSL source assembly, offline validation and diagnostics.
//!
//! Shaders are plain WGSL files under `adapters/gpu-world/shaders/`. An [`Assembly`] is an
//! ordered list of modules concatenated into one source, starting with the generated layout
//! declarations ([`crate::layout::wgsl_declarations`]). There are no imports or preprocessor:
//! a module may use anything an earlier module declares. The assembly keeps a line map, so a
//! parse or validation error points at the module file and line it came from.
//!
//! The same assembled source is validated offline (`cargo evidence check static`), compiled at
//! runtime and available to other consumers of the layouts (the renderer, layout probes).

use std::{borrow::Cow, fmt};

/// One WGSL source file.
#[derive(Debug, Clone)]
pub struct SourceModule {
    /// Path relative to `adapters/gpu-world/shaders/`, or `generated/...` for generated text.
    pub path: &'static str,
    pub text: Cow<'static, str>,
}

impl SourceModule {
    pub const fn file(path: &'static str, text: &'static str) -> Self {
        Self {
            path,
            text: Cow::Borrowed(text),
        }
    }
}

/// The generated layout declarations.
pub fn generated_declarations() -> SourceModule {
    SourceModule {
        path: "generated/layout.wgsl",
        text: Cow::Owned(crate::layout::wgsl_declarations()),
    }
}

/// Cell field accessors over the generated declarations.
pub fn cell_accessors() -> SourceModule {
    SourceModule::file(
        "shared/cell.wgsl",
        include_str!("../shaders/shared/cell.wgsl"),
    )
}

/// The modules any consumer of the packed layouts starts with: declarations and accessors.
pub fn shared_modules() -> Vec<SourceModule> {
    vec![generated_declarations(), cell_accessors()]
}

/// The rules as pure functions of cells and positions, without bindings: keyed hashes, draws and
/// shuffles, and the column rules. Rule probes assemble them after [`shared_modules`].
pub fn rule_modules() -> Vec<SourceModule> {
    vec![
        SourceModule::file(
            "shared/rules.wgsl",
            include_str!("../shaders/shared/rules.wgsl"),
        ),
        SourceModule::file(
            "shared/column.wgsl",
            include_str!("../shaders/shared/column.wgsl"),
        ),
    ]
}

fn window_helpers() -> SourceModule {
    SourceModule::file(
        "shared/window.wgsl",
        include_str!("../shaders/shared/window.wgsl"),
    )
}

fn world_bindings() -> SourceModule {
    SourceModule::file(
        "world/bindings.wgsl",
        include_str!("../shaders/world/bindings.wgsl"),
    )
}

fn base_modules(bindings: SourceModule) -> Vec<SourceModule> {
    let mut modules = shared_modules();
    modules.extend(rule_modules());
    modules.push(bindings);
    modules.push(window_helpers());
    modules
}

/// The command kernels: `begin_tick`, `apply_edit`, `query_column` and `measure_totals`.
pub fn world_assembly() -> Assembly {
    let mut modules = base_modules(world_bindings());
    modules.extend([
        SourceModule::file(
            "world/tick.wgsl",
            include_str!("../shaders/world/tick.wgsl"),
        ),
        SourceModule::file(
            "world/edit.wgsl",
            include_str!("../shaders/world/edit.wgsl"),
        ),
        SourceModule::file(
            "world/query.wgsl",
            include_str!("../shaders/world/query.wgsl"),
        ),
        SourceModule::file(
            "world/totals.wgsl",
            include_str!("../shaders/world/totals.wgsl"),
        ),
    ]);
    Assembly::new("world", &modules)
}

/// The material phases: activity scheduling (`scan_tiles`, `candidate_args`, `wake_tiles`,
/// `process_args`) and the passes `propose`, `resolve`, `react` and `commit`.
pub fn sim_assembly() -> Assembly {
    let mut modules = base_modules(SourceModule::file(
        "sim/bindings.wgsl",
        include_str!("../shaders/sim/bindings.wgsl"),
    ));
    modules.extend([
        SourceModule::file(
            "sim/schedule.wgsl",
            include_str!("../shaders/sim/schedule.wgsl"),
        ),
        SourceModule::file("sim/flow.wgsl", include_str!("../shaders/sim/flow.wgsl")),
        SourceModule::file("sim/react.wgsl", include_str!("../shaders/sim/react.wgsl")),
    ]);
    Assembly::new("sim", &modules)
}

/// The pressure-injection kernel `flood_events`, compiled only with fault injection.
pub fn fault_assembly() -> Assembly {
    let mut modules = base_modules(world_bindings());
    modules.push(SourceModule::file(
        "faults/flood.wgsl",
        include_str!("../shaders/faults/flood.wgsl"),
    ));
    Assembly::new("faults", &modules)
}

/// Every assembly this crate compiles.
pub fn production_assemblies() -> Vec<Assembly> {
    vec![world_assembly(), sim_assembly(), fault_assembly()]
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ModuleSpan {
    path: &'static str,
    /// First line of the module's own text in the assembled source, 1-based.
    first_line: u32,
    lines: u32,
}

/// An assembled shader source and the map back to its modules.
#[derive(Debug, Clone)]
pub struct Assembly {
    name: &'static str,
    source: String,
    spans: Vec<ModuleSpan>,
}

/// A location in a module file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShaderDiagnostic {
    pub assembly: &'static str,
    /// The module file, or `<assembly>` when the location could not be mapped.
    pub module: &'static str,
    pub line: u32,
    pub column: u32,
    pub message: String,
}

impl fmt::Display for ShaderDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}:{} ({}): {}",
            self.module, self.line, self.column, self.assembly, self.message
        )
    }
}

impl Assembly {
    pub fn new(name: &'static str, modules: &[SourceModule]) -> Self {
        let mut source = String::new();
        let mut spans = Vec::with_capacity(modules.len());
        let mut line = 1u32;
        for module in modules {
            source.push_str("// ---- module ");
            source.push_str(module.path);
            source.push('\n');
            line += 1;
            let text = module.text.as_ref();
            let lines = text.lines().count() as u32;
            spans.push(ModuleSpan {
                path: module.path,
                first_line: line,
                lines,
            });
            source.push_str(text);
            if !text.ends_with('\n') {
                source.push('\n');
            }
            line += lines;
        }
        Self {
            name,
            source,
            spans,
        }
    }

    pub fn name(&self) -> &'static str {
        self.name
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    /// The module and module-relative line of an assembled line.
    pub fn locate(&self, line: u32) -> Option<(&'static str, u32)> {
        self.spans
            .iter()
            .find(|span| line >= span.first_line && line < span.first_line + span.lines)
            .map(|span| (span.path, line - span.first_line + 1))
    }

    fn diagnostic(
        &self,
        location: Option<naga::SourceLocation>,
        message: String,
    ) -> ShaderDiagnostic {
        let (module, line, column) = match location {
            Some(location) => match self.locate(location.line_number) {
                Some((module, line)) => (module, line, location.line_position),
                None => ("<assembly>", location.line_number, location.line_position),
            },
            None => ("<assembly>", 0, 0),
        };
        ShaderDiagnostic {
            assembly: self.name,
            module,
            line,
            column,
            message,
        }
    }

    /// Parses and validates the assembled source as wgpu would, with every validation check
    /// enabled.
    pub fn validate(&self) -> Result<(), ShaderDiagnostic> {
        let module = naga::front::wgsl::parse_str(&self.source).map_err(|error| {
            self.diagnostic(
                error.location(&self.source),
                error.emit_to_string(&self.source),
            )
        })?;
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::default(),
        )
        .validate(&module)
        .map_err(|error| {
            self.diagnostic(
                error.location(&self.source),
                error.emit_to_string(&self.source),
            )
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_production_assembly_validates() {
        for assembly in production_assemblies() {
            if let Err(diagnostic) = assembly.validate() {
                panic!("{diagnostic}");
            }
        }
    }

    #[test]
    fn errors_point_at_the_module_file_and_line() {
        let broken = SourceModule::file(
            "probe/broken.wgsl",
            "fn ok() {}\n\nfn broken() -> u32 {\n    return missing_value;\n}\n",
        );
        let mut modules = shared_modules();
        modules.push(broken);
        let assembly = Assembly::new("specimen", &modules);
        let diagnostic = assembly
            .validate()
            .expect_err("an undefined name is an error");
        assert_eq!(diagnostic.module, "probe/broken.wgsl");
        assert_eq!(diagnostic.line, 4);
        assert!(diagnostic.message.contains("missing_value"), "{diagnostic}");
    }

    #[test]
    fn the_line_map_covers_each_module_once() {
        let assembly = world_assembly();
        let lines = assembly.source().lines().count() as u32;
        let mut mapped = 0;
        for line in 1..=lines {
            if assembly.locate(line).is_some() {
                mapped += 1;
            }
        }
        // Every line except the one marker line per module belongs to exactly one module.
        assert_eq!(mapped, lines - assembly.spans.len() as u32);
        assert_eq!(assembly.locate(2), Some(("generated/layout.wgsl", 1)));
    }
}
