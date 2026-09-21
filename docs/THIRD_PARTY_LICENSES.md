# Third-Party Licenses

OpenBNCT does not vendor third-party source, transport engines, nuclear data,
or patient data. Rust packages are resolved from crates.io and locked by the
committed `Cargo.lock`.

The current direct third-party packages are:

| Package | Resolved version | Declared license |
| --- | ---: | --- |
| base64 | 0.22.1 | MIT OR Apache-2.0 |
| clap | 4.6.6 | MIT OR Apache-2.0 |
| dicom-core | 0.10.0 | MIT OR Apache-2.0 |
| dicom-dictionary-std | 0.10.0 | MIT OR Apache-2.0 |
| dicom-object | 0.10.0 | MIT OR Apache-2.0 |
| eframe | 0.36.1 | MIT OR Apache-2.0 |
| image | 0.25.10 | MIT OR Apache-2.0 |
| md-5 | 0.10.6 | MIT OR Apache-2.0 |
| quick-xml | 0.42.0 | MIT |
| reqwest | 0.13.4 | MIT OR Apache-2.0 |
| serde | 1.0.229 | MIT OR Apache-2.0 |
| serde_json | 1.0.151 | MIT OR Apache-2.0 |
| sha2 | 0.10.9 | MIT OR Apache-2.0 |
| thiserror | 2.0.20 | MIT OR Apache-2.0 |
| uuid | 1.26.0 | Apache-2.0 OR MIT |
| tempfile (development only) | 3.27.0 | MIT OR Apache-2.0 |

CI installs, but does not vendor, link, or redistribute, dicom3tools snapshot
`20240118131615` under BSD-3-Clause solely to run `dciodvfy` and `dcentvfy` as
independent DICOM validation tools.

The optional nuclear-data inspection script requires a user-supplied Python,
NumPy (BSD-3-Clause and compatible notices), h5py (BSD-3-Clause), and HDF5
(BSD-style) environment. Its Python packages are pinned in a dedicated
requirements file, and all exact runtime versions are recorded in every
generated manifest; they are not bundled by OpenBNCT.

This table records direct packages, not a substitute for the complete
transitive license and notice bundle. CI must generate and review that bundle
from `Cargo.lock` before the first binary or archival release.

OpenMC is a planned external transport backend and is not part of OpenBNCT.
OpenBNCT's acquisition client does not redistribute OpenMC or nuclear data;
users remain responsible for obtaining and validating the external engine and
data libraries used by their calculations.
