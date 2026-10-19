//! The renderer's WGSL assemblies. Each starts with the GPU owner's generated declarations and
//! cell accessors (`gpu_world::shader::shared_modules`), so the packed cell layout, the window
//! map's codes and the body records have one owner; the renderer adds only its own passes.

use std::borrow::Cow;

use gpu_world::shader::{self, Assembly, SourceModule};

fn module(path: &'static str, text: &'static str) -> SourceModule {
    SourceModule::file(path, text)
}

fn constants() -> SourceModule {
    SourceModule {
        path: "generated/render_constants.wgsl",
        text: Cow::Owned(crate::lighting::wgsl_constants()),
    }
}

fn types() -> SourceModule {
    module("render/types.wgsl", include_str!("../shaders/types.wgsl"))
}

fn body_place() -> SourceModule {
    module(
        "render/body_place.wgsl",
        include_str!("../shaders/body_place.wgsl"),
    )
}

/// The landscape layers, the spheres and the sphere shadow pass.
pub fn scene_assembly() -> Assembly {
    let mut modules = shader::shared_modules();
    modules.extend([
        shader::body_read_access(),
        constants(),
        types(),
        module(
            "render/bindings.wgsl",
            include_str!("../shaders/bindings.wgsl"),
        ),
        module("render/world.wgsl", include_str!("../shaders/world.wgsl")),
        body_place(),
        module(
            "render/lighting.wgsl",
            include_str!("../shaders/lighting.wgsl"),
        ),
        module(
            "render/terrain.wgsl",
            include_str!("../shaders/terrain.wgsl"),
        ),
        module("render/bodies.wgsl", include_str!("../shaders/bodies.wgsl")),
    ]);
    Assembly::new("render scene", &modules)
}

/// The camera kernel (`place_camera`).
pub fn camera_assembly() -> Assembly {
    let mut modules = shader::shared_modules();
    modules.extend([
        shader::body_read_access(),
        constants(),
        types(),
        module(
            "render/camera_bindings.wgsl",
            include_str!("../shaders/camera_bindings.wgsl"),
        ),
        body_place(),
        module("render/camera.wgsl", include_str!("../shaders/camera.wgsl")),
    ]);
    Assembly::new("render camera", &modules)
}

/// Bloom and tone mapping.
pub fn post_assembly() -> Assembly {
    Assembly::new(
        "render post",
        &[module(
            "render/post.wgsl",
            include_str!("../shaders/post.wgsl"),
        )],
    )
}

/// Every assembly the renderer compiles, for offline validation (`cargo evidence check static`).
pub fn production_assemblies() -> Vec<Assembly> {
    vec![scene_assembly(), camera_assembly(), post_assembly()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_render_assembly_validates() {
        for assembly in production_assemblies() {
            if let Err(diagnostic) = assembly.validate() {
                panic!("{diagnostic}");
            }
        }
    }
}
