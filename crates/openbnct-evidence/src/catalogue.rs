// SPDX-License-Identifier: MIT

//! Versioned benchmark catalogue (`openbnct.benchmark-catalogue/0.1.0`)
//! and its verification report (`openbnct.benchmark-report/0.1.0`).
//!
//! The catalogue *references* the immutable evidence under
//! `benchmarks/` and `validation/`; it never rewrites it. Each entry
//! declares its problem class, evidence kinds, tolerances (with when
//! they were set — a tolerance invented after the fact is recorded as
//! `documented_after`, never silently promoted to `preregistered`),
//! graded versus excluded regions, license, uncertainty treatment, and
//! limitations. Broken references, absent licenses, and absent
//! uncertainty statements are *reported* by [`verify_catalogue`]; they
//! are not filled with defaults.
//!
//! An outside implementation consumes the catalogue as plain JSON:
//! entry paths are repository-relative, artifact hashes are SHA-256,
//! and no OpenBNCT engine is required to read or score against the
//! declared references.

use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};

use openbnct_core::ContentReference;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{QualificationBoundary, sha256_file, validate_relative_path};

/// Contract token for [`BenchmarkCatalogue`].
pub const BENCHMARK_CATALOGUE_SCHEMA: &str = "openbnct.benchmark-catalogue/0.1.0";
/// Contract token for [`BenchmarkReport`].
pub const BENCHMARK_REPORT_SCHEMA: &str = "openbnct.benchmark-report/0.1.0";

/// What a catalogued case exercises.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProblemClass {
    /// Particle transport / flux-dose solving.
    Transport,
    /// Dose folding, scoring, component decomposition.
    Dosimetry,
    /// Plan optimization or evaluation.
    Planning,
    /// Pharmacokinetic scheduling/integration.
    Pharmacokinetic,
    /// Biological-weighted or microdosimetric endpoints.
    BiologicalResponse,
    /// Detector/instrument geometry or response models.
    Instrumentation,
}

/// Where an entry's evidence comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    /// Closed-form ground truth (exact equation evaluated on the case).
    Analytic,
    /// A published numerical reference (table, benchmark report).
    PublishedNumerical,
    /// An independent engine/code solving the same case.
    IndependentEngine,
    /// A measured experiment (digitized or instrumented data).
    MeasuredExperiment,
    /// Internal self-consistency (superposition, symmetry, balance).
    InternalClosure,
    /// A declared-input study — published ranges/anchors encoded as
    /// model inputs and exercised end to end.
    DeclaredInputStudy,
    /// A parser/format conformance fixture.
    ParserFixture,
}

/// Outcome of one evidence item — failures and unexecuted items are
/// first-class, never dropped to make a case look uniform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceVerdict {
    Pass,
    Fail,
    /// Discrepancy documented as expected physics or a declared
    /// limitation (e.g. the Kobayashi deep-field ray effect) — reported
    /// but not graded as pass/fail.
    Documented,
    /// Evaluation ran but did not cover the full declared scope.
    Incomplete,
    /// Item exists but does not apply to this case.
    NotApplicable,
    /// Declared but never executed.
    Unexecuted,
}

/// When the tolerance was set — a historical tolerance must not be
/// retroactively described as preregistered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToleranceDeclared {
    /// Frozen before the result was produced.
    Preregistered,
    /// Recorded when the evidence was generated.
    AtExecution,
    /// Declared after the evidence existed.
    DocumentedAfter,
}

/// One scored region's tolerance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogueTolerance {
    /// What the tolerance covers — a region, probe set, or metric.
    pub scope: String,
    /// The criterion as declared (e.g. `5% relative`, `2.0σ`).
    pub criterion: String,
    pub declared: ToleranceDeclared,
}

/// One evidence item inside a catalogue entry — a comparison artifact,
/// an oracle evaluation, a measurement anchor, or a recorded failure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceItem {
    pub kind: EvidenceKind,
    /// Short label ("near-field probes", "peak-normalized water depth").
    pub label: String,
    /// Artifact carrying the graded result, relative to `entry.path`.
    /// `None` for a status note that references no file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// Lowercase hex SHA-256 of `file` — required when `file` is set,
    /// so a changed artifact is caught by `bench verify`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// The spatial or metric region this item covers, when the entry
    /// grades regions separately (e.g. near-field vs ray-effect regime).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    pub verdict: EvidenceVerdict,
    /// Where the reference comes from (citation, engine + version,
    /// measurement campaign).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    /// How the comparison is normalized — `peak-normalized shape only`
    /// comparisons must say so and cannot appear as absolute-dose
    /// agreement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub normalization: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Execution state of the whole entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryStatus {
    /// The declared evidence was produced and recorded.
    Executed,
    /// Executed and passing its gates, but not yet promoted to
    /// reference status (e.g. awaiting cross-code reproduction).
    Candidate,
    /// Inputs declared; no result evidence exists yet.
    Unexecuted,
}

/// One catalogued case directory.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogueEntry {
    /// Stable identifier (`nf-bnct-001`, `canonical-kobayashi-p1`).
    pub id: String,
    pub title: String,
    /// Case directory, relative to the repository/catalogue root.
    pub path: String,
    pub problem_class: ProblemClass,
    /// `synthetic` | `canonical-literature` | `measured-phantom` |
    /// `intercomparison` — the broad evidence family.
    pub modality: String,
    pub qualification: QualificationBoundary,
    pub status: EntryStatus,
    /// Solver/data description where execution is relevant
    /// ("S8 diamond-difference, 28-group ENDF/B-VIII.1 collapse").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solver: Option<String>,
    /// Data license covering the entry's committed inputs/references.
    /// Absent licenses are reported by verification, not defaulted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    /// Declared uncertainty treatment for the entry's evidence.
    /// Absent statements are reported by verification, not defaulted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uncertainty: Option<String>,
    /// What the reference results are normalized to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub normalization: Option<String>,
    #[serde(default)]
    pub tolerances: Vec<CatalogueTolerance>,
    pub evidence: Vec<EvidenceItem>,
    /// Honest limitations carried on the record.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub limitations: Vec<String>,
}

/// The catalogue document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BenchmarkCatalogue {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    pub title: String,
    /// Free-text revision note (source commit or generation command).
    pub revision: String,
    pub qualification: QualificationBoundary,
    pub provenance_id: String,
    pub entries: Vec<CatalogueEntry>,
}

/// Severity of a [`CatalogueFinding`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingSeverity {
    /// Structural problem: missing file, hash mismatch, inconsistency.
    Error,
    /// Metadata gap: absent license or uncertainty statement.
    Warning,
}

/// One verification observation — surfaced, never auto-repaired.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogueFinding {
    pub severity: FindingSeverity,
    /// Entry id the finding belongs to (or `""` for catalogue-level).
    pub entry: String,
    pub message: String,
}

/// Read-only verification result artifact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BenchmarkReport {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    /// Content binding to the verified catalogue document.
    pub catalogue: ContentReference,
    pub entries_checked: usize,
    /// Count of error-severity findings — nonzero means verification failed.
    pub errors: usize,
    pub warnings: usize,
    pub findings: Vec<CatalogueFinding>,
    pub qualification: QualificationBoundary,
    pub provenance_id: String,
}

#[derive(Debug, Error)]
pub enum CatalogueError {
    #[error("unsupported catalogue schema {0:?}")]
    UnsupportedSchema(String),
    #[error("invalid catalogue: {0}")]
    Invalid(String),
    #[error("catalogue path escapes the evidence root: {0}")]
    PathEscapesRoot(String),
    #[error("I/O failed for {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

impl BenchmarkCatalogue {
    pub fn validate(&self) -> Result<(), CatalogueError> {
        if !openbnct_core::schema_matches(&self.schema_version, BENCHMARK_CATALOGUE_SCHEMA) {
            return Err(CatalogueError::UnsupportedSchema(
                self.schema_version.clone(),
            ));
        }
        for (label, value) in [
            ("id", self.id.as_str()),
            ("title", self.title.as_str()),
            ("provenance_id", self.provenance_id.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(CatalogueError::Invalid(format!("{label} is empty")));
            }
        }
        let mut ids = BTreeSet::new();
        for entry in &self.entries {
            if entry.id.trim().is_empty() || entry.title.trim().is_empty() {
                return Err(CatalogueError::Invalid(
                    "entry id and title are required".into(),
                ));
            }
            if !ids.insert(entry.id.as_str()) {
                return Err(CatalogueError::Invalid(format!(
                    "duplicate entry id {:?}",
                    entry.id
                )));
            }
            validate_relative_path(&entry.path)
                .map_err(|e| CatalogueError::Invalid(e.to_string()))?;
            let mut regions = BTreeSet::new();
            for tolerance in &entry.tolerances {
                if tolerance.scope.trim().is_empty() || tolerance.criterion.trim().is_empty() {
                    return Err(CatalogueError::Invalid(format!(
                        "entry {:?} tolerance needs scope and criterion",
                        entry.id
                    )));
                }
                regions.insert(tolerance.scope.as_str());
            }
            for item in &entry.evidence {
                if item.label.trim().is_empty() {
                    return Err(CatalogueError::Invalid(format!(
                        "entry {:?} carries an unlabeled evidence item",
                        entry.id
                    )));
                }
                match (&item.file, &item.sha256) {
                    (Some(file), Some(hash)) => {
                        validate_relative_path(file)
                            .map_err(|e| CatalogueError::Invalid(e.to_string()))?;
                        if hash.len() != 64
                            || !hash
                                .bytes()
                                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
                        {
                            return Err(CatalogueError::Invalid(format!(
                                "entry {:?} item {:?}: sha256 must be lowercase hex",
                                entry.id, item.label
                            )));
                        }
                    }
                    (Some(file), None) => {
                        return Err(CatalogueError::Invalid(format!(
                            "entry {:?} item {:?} references {file:?} without a sha256",
                            entry.id, item.label
                        )));
                    }
                    (None, Some(_)) => {
                        return Err(CatalogueError::Invalid(format!(
                            "entry {:?} item {:?} declares a sha256 without a file",
                            entry.id, item.label
                        )));
                    }
                    (None, None) => {}
                }
            }
            // An unexecuted entry may not carry a passed/failed result:
            // declared-but-never-run evidence stays `unexecuted`.
            if entry.status == EntryStatus::Unexecuted
                && entry.evidence.iter().any(|item| {
                    matches!(
                        item.verdict,
                        EvidenceVerdict::Pass | EvidenceVerdict::Fail | EvidenceVerdict::Documented
                    )
                })
            {
                return Err(CatalogueError::Invalid(format!(
                    "entry {:?} is unexecuted but carries graded evidence",
                    entry.id
                )));
            }
        }
        Ok(())
    }
}

impl BenchmarkReport {
    pub fn validate(&self) -> Result<(), CatalogueError> {
        if !openbnct_core::schema_matches(&self.schema_version, BENCHMARK_REPORT_SCHEMA) {
            return Err(CatalogueError::UnsupportedSchema(
                self.schema_version.clone(),
            ));
        }
        if self.id.trim().is_empty() {
            return Err(CatalogueError::Invalid("id is empty".into()));
        }
        self.catalogue
            .validate()
            .map_err(|e| CatalogueError::Invalid(e.to_string()))?;
        if self.errors
            != self
                .findings
                .iter()
                .filter(|f| f.severity == FindingSeverity::Error)
                .count()
            || self.warnings
                != self
                    .findings
                    .iter()
                    .filter(|f| f.severity == FindingSeverity::Warning)
                    .count()
        {
            return Err(CatalogueError::Invalid(
                "error/warning counts do not match findings".into(),
            ));
        }
        Ok(())
    }
}

fn finding(severity: FindingSeverity, entry: &str, message: String) -> CatalogueFinding {
    CatalogueFinding {
        severity,
        entry: entry.into(),
        message,
    }
}

/// Read-only verification: resolves every referenced file under `root`
/// (canonicalized, no escape), re-hashes artifacts that declare a
/// sha256, and reports absent licenses/uncertainties. Nothing on disk
/// is written or modified.
pub fn verify_catalogue(
    catalogue: &BenchmarkCatalogue,
    root: &Path,
) -> Result<Vec<CatalogueFinding>, CatalogueError> {
    catalogue.validate()?;
    let canonical_root = std::fs::canonicalize(root).map_err(|source| CatalogueError::Io {
        path: root.to_path_buf(),
        source,
    })?;
    let mut findings = Vec::new();
    for entry in &catalogue.entries {
        let dir = canonical_root.join(&entry.path);
        let resolved_dir = match std::fs::canonicalize(&dir) {
            Ok(path) => path,
            Err(source) => {
                findings.push(finding(
                    FindingSeverity::Error,
                    &entry.id,
                    format!(
                        "case directory {:?} cannot be resolved: {source}",
                        entry.path
                    ),
                ));
                continue;
            }
        };
        if !resolved_dir.starts_with(&canonical_root) {
            return Err(CatalogueError::PathEscapesRoot(entry.path.clone()));
        }
        if !resolved_dir.is_dir() {
            findings.push(finding(
                FindingSeverity::Error,
                &entry.id,
                format!("case directory {:?} is not a directory", entry.path),
            ));
            continue;
        }
        if entry.license.is_none() {
            findings.push(finding(
                FindingSeverity::Warning,
                &entry.id,
                "no data license declared".into(),
            ));
        }
        if entry.uncertainty.is_none() {
            findings.push(finding(
                FindingSeverity::Warning,
                &entry.id,
                "no uncertainty statement declared".into(),
            ));
        }
        for item in &entry.evidence {
            let (Some(file), Some(expected)) = (&item.file, &item.sha256) else {
                continue;
            };
            let unresolved = resolved_dir.join(file);
            let resolved = match std::fs::canonicalize(&unresolved) {
                Ok(path) => path,
                Err(source) => {
                    findings.push(finding(
                        FindingSeverity::Error,
                        &entry.id,
                        format!(
                            "item {:?}: {file:?} cannot be resolved: {source}",
                            item.label
                        ),
                    ));
                    continue;
                }
            };
            // Evidence files live inside their case directory — a link
            // or `..`-free path that still lands elsewhere (e.g. a
            // sibling of the entry) is an escape.
            if !resolved.starts_with(&canonical_root) || !resolved.starts_with(&resolved_dir) {
                return Err(CatalogueError::PathEscapesRoot(format!(
                    "{}/{}",
                    entry.path, file
                )));
            }
            if !resolved.is_file() {
                findings.push(finding(
                    FindingSeverity::Error,
                    &entry.id,
                    format!("item {:?}: {file:?} is not a regular file", item.label),
                ));
                continue;
            }
            let observed = sha256_file(&resolved).map_err(|source| CatalogueError::Io {
                path: resolved.clone(),
                source,
            })?;
            if observed != *expected {
                findings.push(finding(
                    FindingSeverity::Error,
                    &entry.id,
                    format!(
                        "item {:?}: {file:?} sha256 mismatch (declared {expected}, observed {observed})",
                        item.label
                    ),
                ));
            }
        }
    }
    Ok(findings)
}

/// Assemble the versioned verification report over `findings`.
pub fn catalogue_report(
    catalogue: &BenchmarkCatalogue,
    catalogue_ref: ContentReference,
    findings: Vec<CatalogueFinding>,
    report_id: &str,
    provenance_id: &str,
) -> Result<BenchmarkReport, CatalogueError> {
    let report = BenchmarkReport {
        schema_version: BENCHMARK_REPORT_SCHEMA.into(),
        id: report_id.into(),
        catalogue: catalogue_ref,
        entries_checked: catalogue.entries.len(),
        errors: findings
            .iter()
            .filter(|f| f.severity == FindingSeverity::Error)
            .count(),
        warnings: findings
            .iter()
            .filter(|f| f.severity == FindingSeverity::Warning)
            .count(),
        findings,
        qualification: catalogue.qualification.clone(),
        provenance_id: provenance_id.into(),
    };
    report.validate()?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evidence(
        file: Option<&str>,
        hash: Option<String>,
        verdict: EvidenceVerdict,
    ) -> EvidenceItem {
        EvidenceItem {
            kind: EvidenceKind::PublishedNumerical,
            label: "item".into(),
            file: file.map(str::to_string),
            sha256: hash,
            region: None,
            verdict,
            origin: None,
            normalization: None,
            note: None,
        }
    }

    fn entry(id: &str, items: Vec<EvidenceItem>) -> CatalogueEntry {
        CatalogueEntry {
            id: id.into(),
            title: id.into(),
            path: "case".into(),
            problem_class: ProblemClass::Transport,
            modality: "canonical-literature".into(),
            qualification: QualificationBoundary::SyntheticResearchOnly,
            status: EntryStatus::Executed,
            solver: None,
            license: Some("CC-BY-4.0".into()),
            uncertainty: Some("sigma-weighted".into()),
            normalization: None,
            tolerances: vec![CatalogueTolerance {
                scope: "near-field probes".into(),
                criterion: "5% relative".into(),
                declared: ToleranceDeclared::AtExecution,
            }],
            evidence: items,
            limitations: Vec::new(),
        }
    }

    fn catalogue(entries: Vec<CatalogueEntry>) -> BenchmarkCatalogue {
        BenchmarkCatalogue {
            schema_version: BENCHMARK_CATALOGUE_SCHEMA.into(),
            id: "cat.test".into(),
            title: "test".into(),
            revision: "test".into(),
            qualification: QualificationBoundary::SyntheticResearchOnly,
            provenance_id: "test".into(),
            entries,
        }
    }

    #[test]
    fn validates_and_rejects_duplicates() {
        let cat = catalogue(vec![entry(
            "a",
            vec![evidence(None, None, EvidenceVerdict::Pass)],
        )]);
        cat.validate().unwrap();
        let mut dup = cat.clone();
        dup.entries.push(dup.entries[0].clone());
        assert!(dup.validate().is_err());
    }

    #[test]
    fn evidence_file_requires_sha256() {
        let bad = catalogue(vec![entry(
            "a",
            vec![evidence(Some("result.json"), None, EvidenceVerdict::Pass)],
        )]);
        assert!(bad.validate().is_err());
        let hash_only = catalogue(vec![entry(
            "a",
            vec![evidence(None, Some("a".repeat(64)), EvidenceVerdict::Pass)],
        )]);
        assert!(hash_only.validate().is_err());
    }

    #[test]
    fn unexecuted_entries_cannot_carry_passed_evidence() {
        let mut e = entry("a", vec![evidence(None, None, EvidenceVerdict::Pass)]);
        e.status = EntryStatus::Unexecuted;
        assert!(catalogue(vec![e]).validate().is_err());
        let mut pending = entry("b", vec![evidence(None, None, EvidenceVerdict::Unexecuted)]);
        pending.status = EntryStatus::Unexecuted;
        catalogue(vec![pending]).validate().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn verify_reports_missing_license_and_hash_mismatch() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let case_dir = temp.path().join("case");
        std::fs::create_dir(&case_dir).unwrap();
        std::fs::write(case_dir.join("result.json"), b"data").unwrap();

        let mut e = entry(
            "a",
            vec![evidence(
                Some("result.json"),
                Some("f".repeat(64)),
                EvidenceVerdict::Pass,
            )],
        );
        e.license = None;
        e.uncertainty = None;
        let findings = verify_catalogue(&catalogue(vec![e]), temp.path()).unwrap();
        assert!(
            findings
                .iter()
                .any(|f| f.severity == FindingSeverity::Warning && f.message.contains("license"))
        );
        assert!(
            findings
                .iter()
                .any(|f| f.severity == FindingSeverity::Error && f.message.contains("mismatch"))
        );

        // Correct hash → only the metadata warnings remain.
        let mut e = entry(
            "a",
            vec![evidence(
                Some("result.json"),
                Some(crate::sha256_hex(b"data")),
                EvidenceVerdict::Pass,
            )],
        );
        e.license = None;
        e.uncertainty = None;
        let findings = verify_catalogue(&catalogue(vec![e]), temp.path()).unwrap();
        assert!(
            findings
                .iter()
                .all(|f| f.severity == FindingSeverity::Warning)
        );
        assert_eq!(findings.len(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn verify_rejects_symlink_escape() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().expect("temporary directory");
        let case_dir = temp.path().join("case");
        std::fs::create_dir(&case_dir).unwrap();
        let outside = temp.path().join("outside.json");
        std::fs::write(&outside, b"outside").unwrap();
        symlink(&outside, case_dir.join("linked.json")).unwrap();

        let e = entry(
            "a",
            vec![evidence(
                Some("linked.json"),
                Some(crate::sha256_hex(b"outside")),
                EvidenceVerdict::Pass,
            )],
        );
        assert!(matches!(
            verify_catalogue(&catalogue(vec![e]), temp.path()),
            Err(CatalogueError::PathEscapesRoot(_))
        ));
    }

    /// `schemas/registry.json` mirrors every `pub const .*_SCHEMA`
    /// token declared in the crates — a contract added or moved without
    /// regenerating the registry fails this test.
    #[test]
    fn schema_registry_mirrors_crate_constants() {
        use std::path::PathBuf;
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let registry: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(root.join("schemas/registry.json")).unwrap(),
        )
        .unwrap();
        // Scan `crates/*/src/*.rs` for `pub const NAME: &str = "openbnct.*"`.
        let mut declared: Vec<(String, String, String, String)> = Vec::new();
        for entry in std::fs::read_dir(root.join("crates")).unwrap().flatten() {
            let src = entry.path().join("src");
            if !src.is_dir() {
                continue;
            }
            for file in std::fs::read_dir(&src).unwrap().flatten() {
                let path = file.path();
                if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                    continue;
                }
                let text = std::fs::read_to_string(&path).unwrap();
                for line in text.lines() {
                    let line = line.trim();
                    if !line.starts_with("pub const ") || !line.contains(": &str = \"openbnct.") {
                        continue;
                    }
                    let name = line
                        .strip_prefix("pub const ")
                        .unwrap()
                        .split(':')
                        .next()
                        .unwrap()
                        .trim()
                        .to_string();
                    let token = line.split('"').nth(1).unwrap().to_string();
                    // Full token shape `openbnct.<name>/<version>` —
                    // skips prefix constants like `SCHEMA_PREFIX`.
                    if !token.contains('/') || token.contains('*') {
                        continue;
                    }
                    declared.push((
                        token,
                        entry.file_name().to_str().unwrap().to_string(),
                        format!("src/{}.rs", path.file_stem().unwrap().to_str().unwrap()),
                        name,
                    ));
                }
            }
        }
        declared.sort();
        declared.dedup();
        let registered: std::collections::BTreeMap<String, &serde_json::Value> = registry["tokens"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| (e["token"].as_str().unwrap().to_string(), e))
            .collect();
        let mut problems = Vec::new();
        for (token, krate, module, name) in &declared {
            match registered.get(token) {
                None => problems.push(format!("missing {token}")),
                Some(e)
                    if e["crate"].as_str() != Some(krate.as_str())
                        || e["module"].as_str() != Some(module.as_str())
                        || e["constant"].as_str() != Some(name.as_str()) =>
                {
                    problems.push(format!("wrong metadata {token}"))
                }
                _ => {}
            }
        }
        for token in registered.keys() {
            if !declared.iter().any(|(tok, ..)| tok == token) {
                problems.push(format!("stale {token}"));
            }
        }
        assert!(problems.is_empty(), "registry drift: {problems:?}");
    }
}
