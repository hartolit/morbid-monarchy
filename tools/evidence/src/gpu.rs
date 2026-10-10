//! Adapter inventory and GPU lanes.
//!
//! `inspect-adapter` enumerates every adapter on every backend and assigns each a stable
//! identifier; GPU commands then name that identifier explicitly. Nothing here picks a
//! "preferred" GPU. Suites and scenarios that later stages own are registered by name so that
//! requesting them fails explicitly instead of passing an empty selection.

use serde::Serialize;

use crate::{
    cli::GpuSuite,
    report::{Item, State},
};

#[derive(Debug, Clone, Serialize)]
pub struct AdapterRecord {
    /// `<backend>-<vendor>-<device>` in lowercase hex, with `-<n>` when several adapters share
    /// it. Pass this to `--adapter`.
    pub id: String,
    pub name: String,
    pub backend: String,
    pub device_type: String,
    /// True for CPU/software rasterizers, which can add correctness coverage but never qualify
    /// the hardware performance target.
    pub software: bool,
    pub vendor: String,
    pub device: String,
    pub pci_bus: String,
    pub driver: String,
    pub driver_info: String,
    pub features: Vec<String>,
    /// `Limits` field names and values exactly as wgpu reports them.
    pub limits: Vec<(String, String)>,
}

/// Enumerates adapters on all compiled backends.
pub fn enumerate() -> Vec<AdapterRecord> {
    let descriptor = wgpu::InstanceDescriptor {
        backends: wgpu::Backends::all(),
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    };
    let instance = wgpu::Instance::new(descriptor);
    let adapters =
        futures_lite::future::block_on(instance.enumerate_adapters(wgpu::Backends::all()));
    let mut records: Vec<AdapterRecord> = adapters.iter().map(record).collect();
    disambiguate(&mut records);
    records
}

fn record(adapter: &wgpu::Adapter) -> AdapterRecord {
    let info = adapter.get_info();
    let backend = format!("{:?}", info.backend).to_lowercase();
    AdapterRecord {
        id: format!("{backend}-{:04x}-{:04x}", info.vendor, info.device),
        name: info.name.clone(),
        software: matches!(info.device_type, wgpu::DeviceType::Cpu),
        device_type: format!("{:?}", info.device_type),
        vendor: format!("0x{:04x}", info.vendor),
        device: format!("0x{:04x}", info.device),
        pci_bus: info.device_pci_bus_id.clone(),
        driver: info.driver.clone(),
        driver_info: info.driver_info.clone(),
        backend,
        features: adapter
            .features()
            .iter_names()
            .map(|(name, _)| name.to_owned())
            .collect(),
        limits: debug_fields(&format!("{:#?}", adapter.limits())),
    }
}

fn disambiguate(records: &mut [AdapterRecord]) {
    let ids: Vec<String> = records.iter().map(|record| record.id.clone()).collect();
    for (index, record) in records.iter_mut().enumerate() {
        let duplicates = ids.iter().filter(|id| **id == record.id).count();
        if duplicates > 1 {
            let ordinal = ids[..index].iter().filter(|id| **id == record.id).count();
            record.id = format!("{}-{ordinal}", record.id);
        }
    }
}

/// Splits pretty `Debug` output of a flat struct into `(field, value)` pairs.
fn debug_fields(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let (name, value) = line.trim().split_once(": ")?;
            Some((name.to_owned(), value.trim_end_matches(',').to_owned()))
        })
        .collect()
}

pub fn inspect_items(records: &[AdapterRecord]) -> Vec<Item> {
    if records.is_empty() {
        return vec![Item::new(
            "gpu.adapters",
            State::Unavailable,
            "no adapter on any backend; GPU lanes cannot run on this host",
        )];
    }
    let details = records
        .iter()
        .map(|record| {
            format!(
                "{}: {} ({}, {}{}), driver {} {}",
                record.id,
                record.name,
                record.backend,
                record.device_type,
                if record.software { ", software" } else { "" },
                record.driver,
                record.driver_info
            )
        })
        .collect();
    vec![
        Item::new(
            "gpu.adapters",
            State::Passed,
            format!("{} adapter(s) enumerated", records.len()),
        )
        .with_details(details),
    ]
}

/// Resolves `--adapter`. A missing adapter is unavailable, not a pass.
pub fn resolve<'a>(records: &'a [AdapterRecord], id: &str) -> Result<&'a AdapterRecord, Item> {
    records
        .iter()
        .find(|record| record.id == id)
        .ok_or_else(|| {
            let known: Vec<String> = records.iter().map(|record| record.id.clone()).collect();
            Item::new(
                "gpu.adapter",
                State::Unavailable,
                format!("adapter `{id}` not found; run `cargo evidence inspect-adapter`"),
            )
            .with_details(known)
        })
}

/// Executes a GPU suite. No suite is implemented before its owner stage; each fails
/// explicitly so a requested GPU lane can never pass on an empty selection.
pub fn suite_items(suite: GpuSuite, adapter: &AdapterRecord) -> Vec<Item> {
    let suites: Vec<GpuSuite> = match suite {
        GpuSuite::All => GpuSuite::NAMED.to_vec(),
        single => vec![single],
    };
    suites
        .into_iter()
        .map(|suite| {
            Item::new(
                format!("gpu.suite.{}", suite.name()),
                State::Failed,
                format!(
                    "suite `{}` is not implemented; prompt {} registers it (adapter {})",
                    suite.name(),
                    suite.owner_stage(),
                    adapter.id
                ),
            )
        })
        .collect()
}

/// Connected scenarios run through the production runtime. They are registered here by the
/// stages that deliver them.
pub fn scenario_items(scenario: &str, adapter: &AdapterRecord) -> Vec<Item> {
    let summary = match scenario {
        "resident-world" => format!(
            "`resident-world` is not implemented; 04–05 build it and 07 completes it (adapter {})",
            adapter.id
        ),
        other => format!("unknown scenario `{other}`; registered: resident-world"),
    };
    vec![Item::new(
        format!("scenario.{scenario}"),
        State::Failed,
        summary,
    )]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(id: &str) -> AdapterRecord {
        AdapterRecord {
            id: id.to_owned(),
            name: String::new(),
            backend: String::new(),
            device_type: String::new(),
            software: false,
            vendor: String::new(),
            device: String::new(),
            pci_bus: String::new(),
            driver: String::new(),
            driver_info: String::new(),
            features: Vec::new(),
            limits: Vec::new(),
        }
    }

    #[test]
    fn duplicate_identifiers_get_ordinals() {
        let mut records = vec![
            named("vulkan-10de-2c05"),
            named("gl-10de-2c05"),
            named("vulkan-10de-2c05"),
        ];
        disambiguate(&mut records);
        let ids: Vec<&str> = records.iter().map(|record| record.id.as_str()).collect();
        assert_eq!(
            ids,
            ["vulkan-10de-2c05-0", "gl-10de-2c05", "vulkan-10de-2c05-1"]
        );
    }

    #[test]
    fn unimplemented_suites_and_scenarios_fail_and_missing_adapters_are_unavailable() {
        let adapter = named("vulkan-10de-2c05");
        for item in suite_items(GpuSuite::All, &adapter) {
            assert_eq!(item.state, State::Failed, "{}", item.id);
        }
        assert_eq!(
            suite_items(GpuSuite::All, &adapter).len(),
            GpuSuite::NAMED.len()
        );
        assert_eq!(
            scenario_items("resident-world", &adapter)[0].state,
            State::Failed
        );
        let missing = resolve(&[adapter], "gl-0000-0000").expect_err("unknown adapter");
        assert_eq!(missing.state, State::Unavailable);
    }

    #[test]
    fn limits_debug_output_becomes_fields() {
        let fields = debug_fields(
            "Limits {\n    max_texture_dimension_1d: 8192,\n    max_bind_groups: 4,\n}",
        );
        assert_eq!(
            fields[0],
            ("max_texture_dimension_1d".to_owned(), "8192".to_owned())
        );
        assert_eq!(fields.len(), 2);
    }
}
