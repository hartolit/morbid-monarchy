use bevy::ecs::resource::Resource;

/// Work the CPU baseline performs, accumulated for measurement.
///
/// Counters observe what the solvers already do; nothing reads them to make a decision. Byte
/// counts are exact sizes of the copies the code performs, not estimates of bus traffic. The
/// systems accept the resource as optional, so worlds built without it behave identically.
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct EngineCounters {
    /// Completed cellular automaton ticks.
    pub ticks: u64,
    /// Simulation time dropped by the catch-up cap, in nanoseconds of simulated time.
    pub discarded_simulation_nanos: u64,
    /// Wall time spent inside `simulate_world`'s tick loop.
    pub simulation_nanos: u64,
    /// Bytes copied by `ActiveWorldGrid::swap_buffers` restoring the front buffer each tick.
    pub full_copy_bytes: u64,
    /// Bytes of wake state reset by `swap_buffers` each tick.
    pub wake_clear_bytes: u64,
    /// Rigid-body substeps integrated.
    pub body_substeps: u64,
    /// Sphere pairs examined by the all-pairs collision pass.
    pub body_pair_tests: u64,
    /// Examined pairs that overlapped and were resolved.
    pub body_pair_contacts: u64,
    /// Wall time spent inside `simulate_rigid_sphere_kinematics`.
    pub body_nanos: u64,
}
