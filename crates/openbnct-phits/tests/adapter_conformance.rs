// SPDX-License-Identifier: MIT

//! Public adapter conformance suite: replays the PHITS cases in
//! `conformance/adapters/0.1.0/manifest.json` through `parse_phits_tally` →
//! `interchange_from_phits` → `import_component_dose` and pins both the
//! generated interchange document and the imported bundle byte-for-byte.
//!
//! Regenerate the references after an intentional adapter change with
//! `OPENBNCT_UPDATE_CONFORMANCE=1 cargo test -p openbnct-phits`.

use std::fs;
use std::path::{Path, PathBuf};

use openbnct_core::{DoseComponent, DoseUnit, import_component_dose};
use openbnct_phits::{ComponentSource, interchange_from_phits};
use serde::Deserialize;
use sha2::Digest;

fn suite_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../conformance/adapters/0.1.0")
        .canonicalize()
        .expect("adapter conformance suite directory")
}

#[derive(Deserialize)]
struct Manifest {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    system: String,
    description: String,
    case_id: String,
    unit: String,
    normalization: String,
    producer_version: Option<String>,
    sources: Vec<Source>,
    document: String,
    bundle: String,
}

#[derive(Deserialize)]
struct Source {
    component: String,
    file: String,
    energy_bin: Option<usize>,
}

fn component(name: &str) -> DoseComponent {
    match name {
        "boron" => DoseComponent::Boron,
        "nitrogen" => DoseComponent::Nitrogen,
        "hydrogen" => DoseComponent::Hydrogen,
        "photon" => DoseComponent::Photon,
        other => panic!("unknown component {other:?}"),
    }
}

fn unit(name: &str) -> DoseUnit {
    match name {
        "gray_per_source_particle" => DoseUnit::GrayPerSourceParticle,
        "gray" => DoseUnit::Gray,
        other => panic!("unknown unit {other:?}"),
    }
}

#[test]
fn phits_adapter_conformance_suite() {
    let root = suite_dir();
    let manifest: Manifest =
        serde_json::from_slice(&fs::read(root.join("manifest.json")).expect("manifest.json"))
            .expect("parse manifest");
    let update = std::env::var_os("OPENBNCT_UPDATE_CONFORMANCE").is_some();
    let mut count = 0;
    // Source file paths are manifest-relative and embedded verbatim in the
    // generated document's normalization trail, so resolve them from root.
    std::env::set_current_dir(&root).expect("enter conformance directory");
    for case in manifest.cases.iter().filter(|c| c.system == "phits") {
        let sources: Vec<ComponentSource> = case
            .sources
            .iter()
            .map(|s| ComponentSource {
                component: component(&s.component),
                file: PathBuf::from(&s.file),
                energy_bin: s.energy_bin,
            })
            .collect();
        let document = interchange_from_phits(
            &sources,
            &case.case_id,
            unit(&case.unit),
            &case.normalization,
            None,
            case.producer_version
                .as_deref()
                .expect("phits names a version"),
        )
        .unwrap_or_else(|e| panic!("{}: {e}", case.description));
        let document_json = serde_json::to_string_pretty(&document).unwrap() + "\n";
        let document_path = root.join(&case.document);
        if update {
            fs::write(&document_path, &document_json).unwrap();
        } else {
            let expected = fs::read_to_string(&document_path)
                .unwrap_or_else(|e| panic!("{}: read document: {e}", case.document));
            assert_eq!(
                document_json, expected,
                "{}: interchange document differs",
                case.document
            );
        }
        let sha256 = format!("{:x}", sha2::Sha256::digest(document_json.as_bytes()));
        let bundle = import_component_dose(&document, &sha256)
            .unwrap_or_else(|e| panic!("{}: import: {e}", case.description));
        let bundle_json = serde_json::to_string_pretty(&bundle).unwrap() + "\n";
        let bundle_path = root.join(&case.bundle);
        if update {
            fs::write(&bundle_path, &bundle_json).unwrap();
        } else {
            let expected = fs::read_to_string(&bundle_path)
                .unwrap_or_else(|e| panic!("{}: read bundle: {e}", case.bundle));
            assert_eq!(
                bundle_json, expected,
                "{}: imported bundle differs",
                case.bundle
            );
        }
        count += 1;
    }
    assert!(count > 0, "suite carries no phits cases");
}
