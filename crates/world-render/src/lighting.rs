//! The scene's light and the landscape palette: the single source of the sun, sky, fog and
//! material looks every pass shades with.
//!
//! Defaults are the baseline's (`WorldLighting` and the landscape palette of prompt 01's
//! visual reference, `docs/development/qualification.md`, "Presentation reference"). Colours
//! are authored in sRGB and shaded in linear light.

use world_model::material::{FluidMat, GranularMat, SurfaceMat, TerrainMat};

/// Palette rows reserved per landscape layer. Materials occupy the lower half of a row and
/// their shading properties the upper half, at [`PALETTE_PROPERTY_OFFSET`].
pub const PALETTE_LAYER_STRIDE: u32 = 32;
pub const PALETTE_PROPERTY_OFFSET: u32 = 16;
const PALETTE_LAYER_COUNT: u32 = 4;
pub const PALETTE_ENTRIES: usize = (PALETTE_LAYER_STRIDE * PALETTE_LAYER_COUNT) as usize;

const LAYER_TERRAIN: u32 = 0;
const LAYER_GRANULAR: u32 = 1;
const LAYER_FLUID: u32 = 2;
const LAYER_SURFACE: u32 = 3;

/// Bevy's `light_consts::lux::AMBIENT_DAYLIGHT`, the baseline sun's illuminance.
pub const AMBIENT_DAYLIGHT_LUX: f32 = 10_000.0;

/// Bevy's default camera exposure (EV100 9.7, `1 / (1.2 * 2^9.7)`), which the baseline's
/// light levels were tuned under.
pub const DEFAULT_EXPOSURE: f32 = 1.0 / (1.2 * 831.746_4);

/// The scene's light. The lighting panel edits the first five fields; the rest are fixed, as in
/// the baseline.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Lighting {
    /// Compass bearing of the sun in degrees: 0 is north (-Z), 90 is east (+X).
    pub sun_azimuth_degrees: f32,
    /// Angle of the sun above the horizon in degrees.
    pub sun_elevation_degrees: f32,
    /// Hemispheric sky light brightness, in the baseline's ambient units.
    pub ambient_brightness: f32,
    /// Opacity of heightfield-marched terrain shadows; zero skips the march.
    pub terrain_shadow_strength: f32,
    pub ambient_occlusion_strength: f32,
    pub sun_illuminance_lux: f32,
    /// sRGB colours.
    pub sun_color: [f32; 3],
    pub sky_color: [f32; 3],
    pub ground_color: [f32; 3],
    /// Horizon haze; also the sky behind the world, so the sky meets the fog.
    pub horizon_color: [f32; 3],
    /// Tint added to the fog when looking towards the sun: sRGB and opacity.
    pub sun_glow_color: [f32; 4],
    pub sun_glow_exponent: f32,
    /// Fog fades in between these fractions of the distance from the camera to the edge of the
    /// resident window, so nearby terrain stays clear while the window boundary is hidden.
    pub fog_start_fraction: f32,
    pub fog_end_fraction: f32,
    pub exposure: f32,
    /// Bloom's blend at the full-resolution mip (Bevy's `Bloom::NATURAL` intensity).
    pub bloom_intensity: f32,
}

/// The lighting panel's ranges (the baseline's).
pub const SUN_AZIMUTH_RANGE: std::ops::RangeInclusive<f32> = 0.0..=360.0;
pub const SUN_ELEVATION_RANGE: std::ops::RangeInclusive<f32> = 2.0..=90.0;
pub const AMBIENT_RANGE: std::ops::RangeInclusive<f32> = 0.0..=3000.0;
pub const UNIT_RANGE: std::ops::RangeInclusive<f32> = 0.0..=1.0;

impl Default for Lighting {
    fn default() -> Self {
        Self {
            sun_azimuth_degrees: 125.0,
            sun_elevation_degrees: 35.0,
            ambient_brightness: 900.0,
            terrain_shadow_strength: 0.85,
            ambient_occlusion_strength: 0.6,
            sun_illuminance_lux: AMBIENT_DAYLIGHT_LUX,
            sun_color: [1.0, 0.95, 0.86],
            sky_color: [0.55, 0.70, 0.95],
            ground_color: [0.42, 0.36, 0.28],
            horizon_color: [0.68, 0.76, 0.86],
            sun_glow_color: [1.0, 0.85, 0.65, 0.5],
            sun_glow_exponent: 24.0,
            fog_start_fraction: 0.55,
            fog_end_fraction: 0.98,
            exposure: DEFAULT_EXPOSURE,
            bloom_intensity: 0.15,
        }
    }
}

impl Lighting {
    /// Unit vector pointing from the ground towards the sun.
    pub fn sun_direction(&self) -> [f32; 3] {
        let azimuth = self.sun_azimuth_degrees.to_radians();
        let elevation = self.sun_elevation_degrees.to_radians();
        [
            elevation.cos() * azimuth.sin(),
            elevation.sin(),
            -elevation.cos() * azimuth.cos(),
        ]
    }

    /// The panel's values clamped into their ranges; a non-finite value takes the default.
    pub fn sanitized(mut self) -> Self {
        let default = Self::default();
        let clamp = |value: f32, range: std::ops::RangeInclusive<f32>, fallback: f32| {
            if value.is_finite() {
                value.clamp(*range.start(), *range.end())
            } else {
                fallback
            }
        };
        self.sun_azimuth_degrees = clamp(
            self.sun_azimuth_degrees,
            SUN_AZIMUTH_RANGE,
            default.sun_azimuth_degrees,
        );
        self.sun_elevation_degrees = clamp(
            self.sun_elevation_degrees,
            SUN_ELEVATION_RANGE,
            default.sun_elevation_degrees,
        );
        self.ambient_brightness = clamp(
            self.ambient_brightness,
            AMBIENT_RANGE,
            default.ambient_brightness,
        );
        self.terrain_shadow_strength = clamp(
            self.terrain_shadow_strength,
            UNIT_RANGE,
            default.terrain_shadow_strength,
        );
        self.ambient_occlusion_strength = clamp(
            self.ambient_occlusion_strength,
            UNIT_RANGE,
            default.ambient_occlusion_strength,
        );
        self
    }
}

/// sRGB to linear light (the sRGB transfer function), as Bevy's `LinearRgba::from` does.
pub fn srgb_to_linear(value: f32) -> f32 {
    if value <= 0.040_45 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

pub fn linear(rgb: [f32; 3]) -> [f32; 3] {
    rgb.map(srgb_to_linear)
}

/// Shading response of one palette material, packed into the property half of its row.
#[derive(Clone, Copy)]
struct MaterialLook {
    srgb: [f32; 3],
    /// Peak sun specular reflectance.
    specular: f32,
    /// Blinn-Phong exponent; higher is glossier.
    shininess: f32,
    /// Self-illumination as a multiple of albedo, added after lighting (feeds bloom).
    emission: f32,
    /// Light absorption per world unit of depth (liquids only); zero is perfectly clear.
    optical_density: f32,
    /// Solid phase of the liquid layer: no ripples or foam, and a crystalline surface instead.
    /// Carried to the shader in the albedo alpha, which is otherwise unused.
    frozen: bool,
}

impl MaterialLook {
    const fn matte(srgb: [f32; 3]) -> Self {
        Self {
            srgb,
            specular: 0.02,
            shininess: 8.0,
            emission: 0.0,
            optical_density: 0.0,
            frozen: false,
        }
    }

    const fn glossy(srgb: [f32; 3], specular: f32, shininess: f32) -> Self {
        Self {
            srgb,
            specular,
            shininess,
            emission: 0.0,
            optical_density: 0.0,
            frozen: false,
        }
    }

    const fn glowing(self, emission: f32) -> Self {
        Self { emission, ..self }
    }

    const fn solid(self) -> Self {
        Self {
            frozen: true,
            ..self
        }
    }

    const fn absorbing(self, optical_density: f32) -> Self {
        Self {
            optical_density,
            ..self
        }
    }
}

/// The landscape palette: per layer and material, the linear albedo with the liquid's mobility
/// in alpha, then the shading properties. Built once per renderer; materials are fixed.
pub fn landscape_palette() -> Vec<[f32; 4]> {
    let entries: [(u32, u8, MaterialLook); 32] = [
        (
            LAYER_TERRAIN,
            TerrainMat::TERRAIN_STONE.0,
            MaterialLook::glossy([0.48, 0.46, 0.44], 0.08, 16.0),
        ),
        (
            LAYER_TERRAIN,
            TerrainMat::TERRAIN_DIRT.0,
            MaterialLook::matte([0.40, 0.28, 0.15]),
        ),
        (
            LAYER_TERRAIN,
            TerrainMat::TERRAIN_SANDSTONE.0,
            MaterialLook::glossy([0.65, 0.55, 0.35], 0.04, 12.0),
        ),
        (
            LAYER_TERRAIN,
            TerrainMat::TERRAIN_DENSE_SNOW.0,
            MaterialLook::glossy([0.70, 0.85, 0.95], 0.25, 32.0),
        ),
        (
            LAYER_TERRAIN,
            TerrainMat::TERRAIN_METAL.0,
            MaterialLook::glossy([0.60, 0.60, 0.65], 0.6, 64.0),
        ),
        (
            LAYER_TERRAIN,
            TerrainMat::TERRAIN_CORRUPTION.0,
            MaterialLook::glossy([0.35, 0.15, 0.40], 0.1, 16.0).glowing(0.05),
        ),
        (
            LAYER_GRANULAR,
            GranularMat::GRANULAR_DIRT.0,
            MaterialLook::matte([0.45, 0.32, 0.18]),
        ),
        (
            LAYER_GRANULAR,
            GranularMat::GRANULAR_SAND.0,
            MaterialLook::glossy([0.82, 0.72, 0.48], 0.05, 16.0),
        ),
        (
            LAYER_GRANULAR,
            GranularMat::GRANULAR_MUD.0,
            MaterialLook::glossy([0.25, 0.18, 0.10], 0.3, 48.0),
        ),
        (
            LAYER_GRANULAR,
            GranularMat::GRANULAR_GRAVEL.0,
            MaterialLook::glossy([0.40, 0.40, 0.42], 0.06, 16.0),
        ),
        (
            LAYER_GRANULAR,
            GranularMat::GRANULAR_SNOW.0,
            MaterialLook::glossy([0.92, 0.94, 0.96], 0.25, 32.0),
        ),
        (
            LAYER_GRANULAR,
            GranularMat::GRANULAR_LIQUID_METAL.0,
            MaterialLook::glossy([0.75, 0.75, 0.80], 0.8, 96.0),
        ),
        (
            LAYER_GRANULAR,
            GranularMat::GRANULAR_CORRUPTION.0,
            MaterialLook::glossy([0.45, 0.20, 0.50], 0.1, 16.0).glowing(0.1),
        ),
        (
            LAYER_FLUID,
            FluidMat::FLUID_WATER.0,
            MaterialLook::glossy([0.10, 0.30, 0.45], 1.0, 256.0).absorbing(0.12),
        ),
        (
            LAYER_FLUID,
            FluidMat::FLUID_MAGMA.0,
            MaterialLook::glossy([0.85, 0.25, 0.05], 0.2, 24.0)
                .glowing(3.0)
                .absorbing(50.0),
        ),
        (
            LAYER_FLUID,
            FluidMat::FLUID_BLOOD.0,
            MaterialLook::glossy([0.55, 0.02, 0.02], 0.6, 96.0).absorbing(1.5),
        ),
        (
            LAYER_FLUID,
            FluidMat::FLUID_ACID.0,
            MaterialLook::glossy([0.30, 0.75, 0.10], 0.8, 128.0)
                .glowing(0.4)
                .absorbing(0.35),
        ),
        (
            LAYER_FLUID,
            FluidMat::FLUID_OIL.0,
            MaterialLook::glossy([0.12, 0.08, 0.04], 1.0, 128.0).absorbing(3.0),
        ),
        (
            LAYER_FLUID,
            FluidMat::FLUID_CORRUPTION.0,
            MaterialLook::glossy([0.25, 0.05, 0.30], 0.6, 64.0)
                .glowing(0.5)
                .absorbing(1.0),
        ),
        (
            LAYER_FLUID,
            FluidMat::FLUID_ICE.0,
            MaterialLook::glossy([0.70, 0.86, 0.97], 0.9, 160.0)
                .absorbing(0.3)
                .solid(),
        ),
        (
            LAYER_SURFACE,
            SurfaceMat::SURFACE_FIRE.0,
            MaterialLook::matte([1.00, 0.60, 0.10]).glowing(4.0),
        ),
        (
            LAYER_SURFACE,
            SurfaceMat::SURFACE_FOLIAGE.0,
            MaterialLook::glossy([0.18, 0.45, 0.12], 0.05, 8.0),
        ),
        (
            LAYER_SURFACE,
            SurfaceMat::SURFACE_WOOD.0,
            MaterialLook::matte([0.45, 0.28, 0.12]),
        ),
        (
            LAYER_SURFACE,
            SurfaceMat::SURFACE_FLESH.0,
            MaterialLook::glossy([0.75, 0.50, 0.45], 0.2, 24.0),
        ),
        (
            LAYER_SURFACE,
            SurfaceMat::SURFACE_BONE.0,
            MaterialLook::glossy([0.88, 0.85, 0.75], 0.1, 16.0),
        ),
        (
            LAYER_SURFACE,
            SurfaceMat::SURFACE_ROT.0,
            MaterialLook::glossy([0.30, 0.22, 0.10], 0.15, 24.0),
        ),
        (
            LAYER_SURFACE,
            SurfaceMat::SURFACE_ASH.0,
            MaterialLook::matte([0.45, 0.42, 0.40]),
        ),
        (
            LAYER_SURFACE,
            SurfaceMat::SURFACE_SNOW.0,
            MaterialLook::glossy([0.95, 0.95, 0.98], 0.25, 32.0),
        ),
        (
            LAYER_SURFACE,
            SurfaceMat::SURFACE_CLAY.0,
            MaterialLook::matte([0.68, 0.42, 0.28]),
        ),
        (
            LAYER_SURFACE,
            SurfaceMat::SURFACE_METAL.0,
            MaterialLook::glossy([0.55, 0.55, 0.60], 0.7, 64.0),
        ),
        (
            LAYER_SURFACE,
            SurfaceMat::SURFACE_GLASS.0,
            MaterialLook::glossy([0.75, 0.88, 0.92], 0.9, 192.0),
        ),
        (
            LAYER_SURFACE,
            SurfaceMat::SURFACE_CORRUPTION.0,
            MaterialLook::glossy([0.50, 0.10, 0.60], 0.2, 24.0).glowing(0.4),
        ),
    ];
    let mut palette = vec![[0.0f32; 4]; PALETTE_ENTRIES];
    for (layer, material, look) in entries {
        let color_slot = (layer * PALETTE_LAYER_STRIDE + u32::from(material)) as usize;
        let property_slot = color_slot + PALETTE_PROPERTY_OFFSET as usize;
        let [red, green, blue] = linear(look.srgb);
        let mobility = if look.frozen { 0.0 } else { 1.0 };
        palette[color_slot] = [red, green, blue, mobility];
        palette[property_slot] = [
            look.specular,
            look.shininess,
            look.emission,
            look.optical_density,
        ];
    }
    palette
}

/// Constants the render shaders add to the GPU owner's generated declarations (which already
/// carry the material ids and rule constants): the palette's layout and the overlay bits,
/// generated so each has one source.
pub fn wgsl_constants() -> String {
    format!(
        "// Generated by world_render::lighting::wgsl_constants. Do not edit.\n\
         const PALETTE_LAYER_STRIDE: u32 = {PALETTE_LAYER_STRIDE}u;\n\
         const PALETTE_PROPERTY_OFFSET: u32 = {PALETTE_PROPERTY_OFFSET}u;\n\
         const PALETTE_ENTRIES: u32 = {PALETTE_ENTRIES}u;\n\
         const OVERLAY_ACTIVITY: u32 = {}u;\n\
         const OVERLAY_BODIES: u32 = {}u;\n",
        crate::frame::OVERLAY_ACTIVITY,
        crate::frame::OVERLAY_BODIES,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_palette_places_every_material_in_its_layer_row() {
        let palette = landscape_palette();
        let water =
            (LAYER_FLUID * PALETTE_LAYER_STRIDE + u32::from(FluidMat::FLUID_WATER.0)) as usize;
        assert!(palette[water][2] > palette[water][0], "water is blue");
        assert_eq!(palette[water][3], 1.0, "water flows");
        let ice = (LAYER_FLUID * PALETTE_LAYER_STRIDE + u32::from(FluidMat::FLUID_ICE.0)) as usize;
        assert_eq!(palette[ice][3], 0.0, "ice is frozen");
        let magma = ice - u32::from(FluidMat::FLUID_ICE.0 - FluidMat::FLUID_MAGMA.0) as usize;
        assert_eq!(
            palette[magma + PALETTE_PROPERTY_OFFSET as usize][2],
            3.0,
            "magma glows"
        );
        // Material ids stay inside their row's colour half.
        for layer in 0..PALETTE_LAYER_COUNT {
            for id in PALETTE_PROPERTY_OFFSET..PALETTE_LAYER_STRIDE {
                let slot = (layer * PALETTE_LAYER_STRIDE + id) as usize;
                let colour = slot - PALETTE_PROPERTY_OFFSET as usize;
                if palette[colour] == [0.0; 4] {
                    assert_eq!(palette[slot], [0.0; 4]);
                }
            }
        }
    }

    #[test]
    fn srgb_conversion_matches_the_transfer_function() {
        assert_eq!(srgb_to_linear(0.0), 0.0);
        assert!((srgb_to_linear(1.0) - 1.0).abs() < 1e-6);
        assert!((srgb_to_linear(0.5) - 0.214_041).abs() < 1e-5);
        assert!((srgb_to_linear(0.04) - 0.04 / 12.92).abs() < 1e-7);
    }

    #[test]
    fn the_sun_points_where_its_bearing_says() {
        let mut lighting = Lighting {
            sun_azimuth_degrees: 90.0,
            sun_elevation_degrees: 0.0,
            ..Lighting::default()
        };
        let [x, y, z] = lighting.sun_direction();
        assert!(
            (x - 1.0).abs() < 1e-6 && y.abs() < 1e-6 && z.abs() < 1e-6,
            "east is +x"
        );
        lighting.sun_azimuth_degrees = 0.0;
        let [_, _, z] = lighting.sun_direction();
        assert!((z + 1.0).abs() < 1e-6, "north is -z");
        let wild = Lighting {
            sun_elevation_degrees: f32::NAN,
            ambient_brightness: 1.0e9,
            ..Lighting::default()
        }
        .sanitized();
        assert_eq!(wild.sun_elevation_degrees, 35.0);
        assert_eq!(wild.ambient_brightness, 3000.0);
    }
}
