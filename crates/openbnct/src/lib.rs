// SPDX-License-Identifier: Apache-2.0

//! Facade crate re-exporting the OpenBNCT library crates under stable
//! namespaced paths (`openbnct::core`, `openbnct::transport`, …).
//!
//! Each subpath is the corresponding `openbnct-*` crate verbatim — this
//! crate adds no API of its own. Prefer depending on the individual
//! crates when you need only part of the workspace; this facade exists
//! for consumers that want one dependency.
//!
//! Research software — no clinical qualification, equivalence,
//! commissioning, or regulatory suitability is claimed. See the
//! repository `docs/DISCLAIMER.md`.

#![forbid(unsafe_code)]

pub use openbnct_bio as bio;
pub use openbnct_boron as boron;
pub use openbnct_core as contracts;
pub use openbnct_dicom as dicom;
pub use openbnct_evidence as evidence;
pub use openbnct_mcnp as mcnp;
pub use openbnct_nifti as nifti;
pub use openbnct_njoy as njoy;
pub use openbnct_openmc as openmc;
pub use openbnct_phits as phits;
pub use openbnct_plan as plan;
pub use openbnct_transport as transport;
pub use openbnct_view as view;
