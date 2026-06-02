# Changelog

- Reworked grid rendering from rigid voxel boxes into a deterministic terrain-style surface: shared corner smoothing, subtle height roughness, planar corner warp, layer boundary skirts, and material mottling now live in `world.wgsl`.
- Added `WorldTuningConfig` controls for `visual_roughness` and `corner_warp`, keeping visual terrain deformation centralized in the app render boundary.
