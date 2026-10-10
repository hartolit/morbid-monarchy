//! Every committed scenario file parses, validates against its profile, and every profile
//! defines every case.

use std::path::Path;

use monarchy_evidence::scenario::{CASES, PROFILES, load, scenario_path};

#[test]
fn committed_scenarios_are_complete_and_valid() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("scenarios");
    for profile in PROFILES {
        for case in CASES {
            let path = scenario_path(&dir, profile.name, case);
            let loaded = load(&path).unwrap_or_else(|error| panic!("{error}"));
            assert_eq!(loaded.scenario.world.seed, 69, "{}", path.display());
        }
    }
    let files = std::fs::read_dir(&dir)
        .expect("scenarios directory")
        .flatten()
        .flat_map(|profile| {
            std::fs::read_dir(profile.path())
                .into_iter()
                .flatten()
                .flatten()
        })
        .count();
    assert_eq!(
        files,
        PROFILES.len() * CASES.len(),
        "unexpected extra scenario files"
    );
}
