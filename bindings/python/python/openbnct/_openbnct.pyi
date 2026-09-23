"""Checked type surface for the compiled ``openbnct._openbnct`` extension.

Every object is immutable. Argument and return shapes mirror the versioned
Rust contracts; ``to_json`` methods return the canonical pretty-printed JSON
serialization produced by the Rust ``serde`` implementation.
"""

from os import PathLike

__version__: str

class NctForgeError(Exception):
    """An NCTForge contract, verification, or evidence check failed."""

class Backend:
    """Transport-backend descriptor with current capability flags."""

    @property
    def id(self) -> str: ...
    @property
    def display_name(self) -> str: ...
    @property
    def version(self) -> str | None: ...
    @property
    def can_prepare(self) -> bool: ...
    @property
    def can_execute(self) -> bool: ...
    @property
    def can_import(self) -> bool: ...

class Structure:
    """Verified ROI summary for a structure in the frozen benchmark case."""

    @property
    def number(self) -> int: ...
    @property
    def name(self) -> str: ...
    @property
    def voxel_count(self) -> int: ...
    @property
    def volume_cm3(self) -> float: ...
    @property
    def centroid_lps_mm(self) -> tuple[float, float, float]: ...

class CaseVerification:
    """Result of the independent ``NF-BNCT-001`` verification oracle."""

    @property
    def case_id(self) -> str: ...
    @property
    def shape(self) -> tuple[int, int, int]:
        """[columns, rows, slices]."""
    @property
    def spacing_mm(self) -> tuple[float, float, float]: ...
    @property
    def origin_mm(self) -> tuple[float, float, float]:
        """LPS position of the centre of voxel [0, 0, 0]."""
    @property
    def ct_slice_count(self) -> int: ...
    @property
    def verified_artifact_count(self) -> int: ...
    @property
    def structures(self) -> list[Structure]: ...

class Geometry:
    """Validated CT lattice geometry in the DICOM LPS patient frame."""

    @property
    def shape(self) -> tuple[int, int, int]: ...
    @property
    def spacing_mm(self) -> tuple[float, float, float]: ...
    @property
    def origin_mm(self) -> tuple[float, float, float]: ...
    @property
    def direction(self) -> tuple[float, ...]: ...
    @property
    def voxel_count(self) -> int: ...
    def voxel_center_lps_mm(self, column: int, row: int, slice: int) -> tuple[float, float, float]: ...

class VerifiedCase:
    """A ``NF-BNCT-001`` case loaded only after every verification gate passed."""

    @property
    def report(self) -> CaseVerification: ...
    @property
    def geometry(self) -> Geometry: ...
    @property
    def frame_of_reference_uid(self) -> str: ...
    @property
    def structures(self) -> list[Structure]: ...
    def ct_value(self, column: int, row: int, slice: int) -> float:
        """Modality value at voxel [column, row, slice], after rescale."""
    def structure_mask(self, name: str) -> list[bool]:
        """Boolean mask for the named structure, columns fastest then rows, slices."""

class GeneratedCase:
    """Summary of a generated ``NF-BNCT-001`` case directory."""

    @property
    def root(self) -> str: ...
    @property
    def ct_file_count(self) -> int: ...
    @property
    def rtstruct_file(self) -> str: ...
    @property
    def manifest_file(self) -> str: ...

class Artifact:
    """One artifact binding inside a verified case manifest."""

    @property
    def role(self) -> str: ...
    @property
    def path(self) -> str: ...
    @property
    def sha256(self) -> str: ...
    @property
    def media_type(self) -> str | None: ...

class CaseManifest:
    """A parsed and schema-validated ``case.json`` manifest."""

    @property
    def schema_version(self) -> str: ...
    @property
    def case_id(self) -> str: ...
    @property
    def qualification(self) -> str: ...
    @property
    def coordinate_system(self) -> str: ...
    @property
    def frame_of_reference_uid(self) -> str: ...
    @property
    def material_model_id(self) -> str: ...
    @property
    def source_model_id(self) -> str: ...
    @property
    def geometry(self) -> Geometry: ...
    @property
    def structures(self) -> list[Structure]: ...
    @property
    def artifacts(self) -> list[Artifact]: ...
    def to_json(self) -> str:
        """Canonical JSON serialization produced by the Rust contract."""
    def verify_artifacts(self, case_root: str | PathLike[str]) -> int:
        """Re-verify every bound artifact; returns the verified count."""

class Material:
    """A validated explicit-nuclide material contract."""

    @property
    def schema_version(self) -> str: ...
    @property
    def id(self) -> str: ...
    def to_json(self) -> str: ...

class FixedSource:
    """A validated backend-neutral fixed-source contract."""

    @property
    def schema_version(self) -> str: ...
    @property
    def id(self) -> str: ...
    def to_json(self) -> str: ...

class ComponentProfile:
    """A validated four-component dose-definition profile."""

    @property
    def schema_version(self) -> str: ...
    @property
    def id(self) -> str: ...
    def to_json(self) -> str: ...

class ResponseGenerationMethod:
    """A validated, versioned response-generation recipe."""

    @property
    def schema_version(self) -> str: ...
    @property
    def id(self) -> str: ...
    def to_json(self) -> str: ...

class ResponseSet:
    """A material-specific neutron response set after base validation.

    ``folding_ready`` additionally requires the independent-review gate used
    before dose folding.
    """

    @property
    def schema_version(self) -> str: ...
    @property
    def id(self) -> str: ...
    @property
    def qualification(self) -> str: ...
    @property
    def folding_ready(self) -> bool: ...
    @property
    def transport_energy_range_ev(self) -> tuple[float, float]: ...
    @property
    def energy_knot_count(self) -> int: ...
    def to_json(self) -> str: ...

def backends() -> list[Backend]:
    """Descriptors for every compiled-in transport backend."""

def file_sha256(path: str | PathLike[str]) -> str:
    """SHA-256 of a file's exact bytes, lowercase hex."""

def generate_case(destination: str | PathLike[str]) -> GeneratedCase:
    """Generate the deterministic synthetic case; refuses existing output."""

def verify_case(root: str | PathLike[str]) -> CaseVerification:
    """Verify a generated case against the independent frozen oracle."""

def load_case(root: str | PathLike[str]) -> VerifiedCase:
    """Load a case only after all geometry and artifact gates pass."""

def read_manifest(path: str | PathLike[str]) -> CaseManifest: ...
def load_material(path: str | PathLike[str]) -> Material: ...
def load_fixed_source(path: str | PathLike[str]) -> FixedSource: ...

class PositionReport:
    """Deterministic report of how a source was positioned on a case."""

    @property
    def schema_version(self) -> str: ...
    @property
    def case_id(self) -> str: ...
    @property
    def target_region(self) -> str: ...
    @property
    def beam_direction_lps(self) -> tuple[float, float, float]: ...
    @property
    def target_centroid_lps_mm(self) -> tuple[float, float, float]: ...
    @property
    def entry(self) -> tuple[str, str]: ...
    @property
    def entry_point_lps_mm(self) -> tuple[float, float, float]: ...
    @property
    def source_to_centroid_mm(self) -> float: ...
    @property
    def aperture_half_widths_cm(self) -> tuple[float, float]: ...
    def to_json(self) -> str: ...
    def write(self, output: str | PathLike[str]) -> None: ...

def aim_source(
    source: FixedSource,
    geometry: Geometry,
    mask: str | PathLike[str],
    case_id: str,
    approach: str | None = None,
    direction_lps: tuple[float, float, float] | None = None,
    half_widths_cm: tuple[float, float] = (1.0, 1.0),
    margin_cm: float = 0.01,
) -> tuple[FixedSource, PositionReport]:
    """Aim a source through a mask's centroid — same path as `position aim`."""

def rotate_source(
    source: FixedSource,
    axis: str,
    center_lps_mm: tuple[float, float, float],
    degrees: float,
) -> FixedSource:
    """Rotate a source about a world axis by a multiple of 90 degrees."""

def load_component_profile(path: str | PathLike[str]) -> ComponentProfile: ...
def load_response_generation_method(path: str | PathLike[str]) -> ResponseGenerationMethod: ...
def load_response_set(path: str | PathLike[str]) -> ResponseSet: ...

class DoseVolume:
    """One component's per-voxel dose values in grid order."""

    @property
    def component(self) -> str: ...
    @property
    def unit(self) -> str: ...
    @property
    def values(self) -> list[float]: ...
    @property
    def absolute_standard_uncertainty(self) -> list[float] | None: ...

class PhysicalDoseBundle:
    """A validated ``openbnct.physical-dose-bundle/0.2.0`` artifact."""

    @property
    def schema_version(self) -> str: ...
    @property
    def case_id(self) -> str: ...
    @property
    def geometry(self) -> Geometry: ...
    @property
    def components(self) -> list[DoseVolume]: ...
    @property
    def physical_total(self) -> DoseVolume:
        """Dedicated physical-total volume, separate from component sums."""
    @property
    def provenance_id(self) -> str: ...
    def to_json(self) -> str: ...

class Exposure:
    """One weighted exposure bound to a dose-bundle file."""

    @property
    def name(self) -> str: ...
    @property
    def dose_bundle_path(self) -> str: ...
    @property
    def dose_bundle_id(self) -> str: ...
    @property
    def dose_bundle_sha256(self) -> str: ...
    @property
    def weight(self) -> float: ...
    @property
    def weight_basis(self) -> str: ...
    @property
    def duration_s(self) -> float | None: ...
    @property
    def boron_assumption(self) -> str | None: ...

class ExposurePlan:
    """A validated ``openbnct.exposure-plan/0.1.0`` weighted-exposure plan."""

    @property
    def schema_version(self) -> str: ...
    @property
    def id(self) -> str: ...
    @property
    def case_id(self) -> str: ...
    @property
    def covariance(self) -> str: ...
    @property
    def exposures(self) -> list[Exposure]: ...
    def validate_diagnostics(self) -> list[str]:
        """Every detectable plan issue, not just the first."""
    def to_json(self) -> str: ...

class BiologicalModel:
    """A validated ``openbnct.biological-model/0.2.0`` artifact."""

    @property
    def schema_version(self) -> str: ...
    @property
    def id(self) -> str: ...
    def to_json(self) -> str: ...

class MicrodosimetricModel:
    """A validated ``openbnct.microdosimetric-model/0.1.0`` artifact (linearized MKM)."""

    @property
    def schema_version(self) -> str: ...
    @property
    def id(self) -> str: ...
    def to_json(self) -> str: ...

class IsoeffectiveModel:
    """A validated ``openbnct.isoeffective-model/0.1.0`` artifact (G&S IsoE dose)."""

    @property
    def schema_version(self) -> str: ...
    @property
    def id(self) -> str: ...
    def to_json(self) -> str: ...

class AppliedFractionation:
    """The fractionation schedule a model applied to a bundle's total."""

    @property
    def fraction_count(self) -> int: ...
    @property
    def source_particles_per_fraction(self) -> float: ...
    @property
    def regions_applied(self) -> list[str]: ...

class BiologicalDoseBundle:
    """A validated biological dose bundle; never aliases physical dose."""

    @property
    def schema_version(self) -> str: ...
    @property
    def case_id(self) -> str: ...
    @property
    def unit(self) -> str:
        """Weighted unit label, deliberately never ``gray``."""
    @property
    def weight_semantics(self) -> str:
        """``fixed_per_component`` or ``photon_isoeffective``."""
    @property
    def fractionation(self) -> AppliedFractionation | None:
        """Applied schedule when the model declared fractionation."""
    @property
    def geometry(self) -> Geometry: ...
    @property
    def components(self) -> list[DoseVolume]: ...
    @property
    def biological_total(self) -> DoseVolume:
        """Weighted total with correlated component-sum uncertainty."""
    @property
    def physical_bundle_provenance(self) -> str: ...
    @property
    def regions_applied(self) -> list[str]: ...
    @property
    def qualification(self) -> str: ...
    def to_json(self) -> str: ...
    def write(self, output: str | PathLike[str]) -> None:
        """Write the bundle JSON; refuses to overwrite an existing file."""

class BioModelComparison:
    """A validated ``openbnct.bio-model-comparison/0.1.0`` artifact."""

    @property
    def schema_version(self) -> str: ...
    @property
    def id(self) -> str: ...
    @property
    def max_voxelwise_ratio(self) -> float | None: ...
    def to_json(self) -> str: ...

class DoseVolumeHistogram:
    """A deterministic ``openbnct.dose-volume-histogram/0.1.0`` artifact."""

    @property
    def schema_version(self) -> str: ...
    @property
    def region(self) -> str: ...
    @property
    def quantity(self) -> str: ...
    @property
    def unit(self) -> str: ...
    @property
    def dose_edges(self) -> list[float]: ...
    @property
    def differential_volume_fraction(self) -> list[float]: ...
    @property
    def cumulative_volume_fraction(self) -> list[float]:
        """V(d): fraction of the region receiving at least each edge dose."""
    @property
    def region_voxel_count(self) -> int: ...
    @property
    def region_volume_mm3(self) -> float: ...
    def to_json(self) -> str: ...

class RegionDoseMetrics:
    """Exact ``openbnct.dose-metrics/0.1.0`` metrics over a voxel mask."""

    @property
    def schema_version(self) -> str: ...
    @property
    def region(self) -> str: ...
    @property
    def quantity(self) -> str: ...
    @property
    def unit(self) -> str: ...
    @property
    def minimum_dose(self) -> float: ...
    @property
    def mean_dose(self) -> float: ...
    @property
    def maximum_dose(self) -> float: ...
    @property
    def dx(self) -> list[tuple[float, float]]:
        """Requested ``D_x`` readings as ``(percent, dose)`` pairs."""
    @property
    def vx(self) -> list[tuple[float, float]]:
        """Requested ``V_x`` readings as ``(level, volume_fraction)`` pairs."""
    @property
    def eud(self) -> list[tuple[float, float]]:
        """Requested EUD readings as ``(a, dose)`` pairs."""
    def to_json(self) -> str: ...

class EndpointModel:
    """A validated ``openbnct.endpoint-model/0.1.0`` artifact."""

    @property
    def schema_version(self) -> str: ...
    @property
    def id(self) -> str: ...
    @property
    def endpoint(self) -> str:
        """``tcp`` or ``ntcp``."""
    def to_json(self) -> str: ...

class AppliedDoseStatistic:
    """The scalar dose statistic a volume-collapsed endpoint consumed."""

    @property
    def kind(self) -> str: ...
    @property
    def parameter(self) -> float | None:
        """EUD organ parameter when ``kind`` is ``eud``."""
    @property
    def value(self) -> float: ...
    @property
    def unit(self) -> str: ...

class EndpointEvaluation:
    """A scored ``openbnct.endpoint-evaluation/0.1.0`` report."""

    @property
    def schema_version(self) -> str: ...
    @property
    def case_id(self) -> str: ...
    @property
    def endpoint(self) -> str:
        """``tcp``, ``ntcp``, or ``utcp``."""
    @property
    def region(self) -> str: ...
    @property
    def quantity(self) -> str: ...
    @property
    def probability(self) -> float: ...
    @property
    def dose_statistic(self) -> AppliedDoseStatistic | None:
        """Absent for ``voxel_poisson_tcp`` and UTCP combinations."""
    @property
    def qualification(self) -> str: ...
    def to_json(self) -> str: ...
    def write(self, output: str | PathLike[str]) -> None:
        """Write the evaluation JSON; refuses to overwrite an existing file."""
class SensitivitySweep:
    """A ``openbnct.bio-sensitivity-sweep/0.1.0`` record."""

    @property
    def schema_version(self) -> str: ...
    @property
    def case_id(self) -> str: ...
    @property
    def region(self) -> str: ...
    @property
    def parameter(self) -> str:
        """Canonical parameter label (``component:boron``, ...)."""
    @property
    def quantity(self) -> str:
        """``biological_total`` or ``weighted_eqd2``."""
    @property
    def unit(self) -> str: ...
    @property
    def model_sha256(self) -> str: ...
    @property
    def dose_bundle_sha256(self) -> str: ...
    @property
    def points(self) -> list[tuple[float, int, float, float, float]]:
        """``(value, region_voxel_count, min, mean, max)`` per point."""
    @property
    def qualification(self) -> str: ...
    def to_json(self) -> str: ...
    def write(self, output: str | PathLike[str]) -> None:
        """Write the sweep JSON; refuses to overwrite an existing file."""

def load_physical_dose_bundle(path: str | PathLike[str]) -> PhysicalDoseBundle: ...
def collect_run(working_directory: str | PathLike[str]) -> PhysicalDoseBundle:
    """Collect a completed OpenMC run directory into a dose bundle."""
def load_biological_model(path: str | PathLike[str]) -> BiologicalModel:
    """Load a ``openbnct.biological-model/0.2.0`` artifact."""
def load_microdosimetric_model(
    path: str | PathLike[str],
) -> MicrodosimetricModel:
    """Load an ``openbnct.microdosimetric-model/0.1.0`` artifact."""
def load_isoeffective_model(path: str | PathLike[str]) -> IsoeffectiveModel:
    """Load an ``openbnct.isoeffective-model/0.1.0`` artifact."""
def load_bio_model_comparison(
    path: str | PathLike[str],
) -> BioModelComparison:
    """Read and validate a ``openbnct.bio-model-comparison/0.1.0`` artifact."""
def make_microdosimetric_model(document: dict | str) -> MicrodosimetricModel:
    """Validate a ``openbnct.microdosimetric-model/0.1.0`` document (dict or JSON)."""
def make_isoeffective_model(document: dict | str) -> IsoeffectiveModel:
    """Validate an ``openbnct.isoeffective-model/0.1.0`` document (dict or JSON)."""
def make_biological_model(document: dict | str) -> BiologicalModel:
    """Validate a ``openbnct.biological-model/0.2.0`` document authored in
    Python (dict or JSON string) into a model object — the
    external-experiment path with validation identical to the file path."""
def apply_model(
    model: BiologicalModel,
    physical: PhysicalDoseBundle,
    region_masks: list[tuple[str, str | PathLike[str]]],
) -> BiologicalDoseBundle:
    """Apply a biological model; region_masks maps region names to mask JSON."""
def apply_mkm_model(
    model: MicrodosimetricModel,
    physical: PhysicalDoseBundle,
    region_masks: list[tuple[str, str | PathLike[str]]],
    spectra: list[str | PathLike[str]],
) -> BiologicalDoseBundle:
    """Apply a linearized-MKM model; spectra are lineal-spectrum JSON paths."""
def apply_isoeffective(
    model: IsoeffectiveModel,
    physical: PhysicalDoseBundle,
    region_masks: list[tuple[str, str | PathLike[str]]],
) -> BiologicalDoseBundle:
    """Apply a González & Santa Cruz photon-isoeffective model."""
def compute_dvh(
    physical: PhysicalDoseBundle,
    quantity: str,
    mask_name: str,
    mask_voxels: list[bool],
    bins: int,
) -> DoseVolumeHistogram:
    """Histogram ``component:NAME`` or ``physical_total`` over the mask."""
def compute_dvh_biological(
    bundle: BiologicalDoseBundle,
    quantity: str,
    mask_name: str,
    mask_voxels: list[bool],
    bins: int,
) -> DoseVolumeHistogram:
    """Histogram ``component:NAME`` or ``biological_total`` over the mask."""
def compute_metrics(
    physical: PhysicalDoseBundle,
    quantity: str,
    mask_name: str,
    mask_voxels: list[bool],
    dx: list[float],
    vx: list[float],
    eud: list[float],
) -> RegionDoseMetrics:
    """Exact D_x/V_x/min/mean/max/EUD metrics over the mask."""
def compute_metrics_biological(
    bundle: BiologicalDoseBundle,
    quantity: str,
    mask_name: str,
    mask_voxels: list[bool],
    dx: list[float],
    vx: list[float],
    eud: list[float],
) -> RegionDoseMetrics:
    """Same as :func:`compute_metrics` for a biological bundle."""
def load_endpoint_model(path: str | PathLike[str]) -> EndpointModel: ...
def load_endpoint_evaluation(path: str | PathLike[str]) -> EndpointEvaluation: ...
def evaluate_endpoint(
    model: EndpointModel,
    physical: PhysicalDoseBundle,
    quantity: str,
    mask_name: str,
    mask_voxels: list[bool],
) -> EndpointEvaluation:
    """Score a TCP/NTCP model over a physical-bundle dose selection."""
def evaluate_endpoint_biological(
    model: EndpointModel,
    bundle: BiologicalDoseBundle,
    quantity: str,
    mask_name: str,
    mask_voxels: list[bool],
) -> EndpointEvaluation:
    """Same as :func:`evaluate_endpoint` for a biological bundle."""
def combine_utcp(
    tcp: EndpointEvaluation,
    ntcp: EndpointEvaluation,
    combination: str,
) -> EndpointEvaluation:
    """Combine TCP and NTCP evaluations; ``p_plus`` or ``difference``."""
def sweep_biological_model(
    model: BiologicalModel,
    physical: PhysicalDoseBundle,
    region_masks: list[tuple[str, str | PathLike[str]]],
    region: str,
    parameter: str,
    values: list[float],
) -> SensitivitySweep:
    """Sweep one model parameter (``component:<name>``,
    ``region_weight:<region>:<component>``, ``alpha_beta:default``,
    ``alpha_beta:<region>``, ``fraction_count``,
    ``source_particles_per_fraction``) over ``values``, recording the
    region-masked min/mean/max of the biological total at each point
    (same path as ``openbnct bio sweep``)."""
def verify_evidence_bundle(root: str | PathLike[str]) -> tuple[str, int]:
    """Re-hash every manifest artifact; returns (case_id, artifact count)."""
def load_exposure_plan(path: str | PathLike[str]) -> ExposurePlan:
    """Load and validate a ``openbnct.exposure-plan/0.1.0`` document."""
def exposure_plan_diagnostics(path: str | PathLike[str]) -> list[str]:
    """Inspect a possibly-malformed plan file and return every detectable
    issue (the counterpart of ``openbnct plan validate``)."""
def accumulate_exposures(plan_path: str | PathLike[str]) -> PhysicalDoseBundle:
    """Run a saved exposure plan end to end — verify bound bundle hashes,
    then accumulate the weighted exposures (same path as ``openbnct
    accumulate``)."""
def plan_table_read(
    table: str | PathLike[str],
    output: str | PathLike[str],
    id: str | None = None,
    case_id: str | None = None,
    bundles_dir: str | PathLike[str] | None = None,
) -> None:
    """Import a ``.csv``/``.xlsx`` exposure table into an exposure-plan JSON
    file at ``output`` (same path as ``openbnct plan import``)."""
def plan_table_write(plan: str | PathLike[str], output: str | PathLike[str]) -> None:
    """Export an exposure-plan JSON file to a ``.csv`` or ``.xlsx`` exposure
    table (same path as ``openbnct plan export``)."""
def import_component_dose(interchange: str | PathLike[str]) -> PhysicalDoseBundle:
    """Import a ``openbnct.component-dose-interchange/0.1.0`` document into a
    validated physical dose bundle (same path as ``openbnct import
    interchange``)."""
def import_mcnp_meshtal(
    components: dict[str, tuple[str | PathLike[str], int] | tuple[str | PathLike[str], int, int]],
    case_id: str,
    unit: str,
    normalization: str,
    producer_version: str | None = None,
    frame_of_reference_uid: str | None = None,
) -> PhysicalDoseBundle:
    """Lift MCNP meshtal component tallies into a physical dose bundle (same
    path as ``openbnct import mcnp``). ``components`` maps each of
    ``boron``/``nitrogen``/``hydrogen``/``photon`` to ``(file, tally)`` or
    ``(file, tally, energy_bin)``."""
def import_phits(
    components: dict[str, str | PathLike[str] | tuple[str | PathLike[str], int]],
    case_id: str,
    unit: str,
    normalization: str,
    producer_version: str,
    frame_of_reference_uid: str | None = None,
) -> PhysicalDoseBundle:
    """Lift PHITS xyz-mesh tally files into a physical dose bundle (same path
    as ``openbnct import phits``). ``components`` maps each component to a
    file path or ``(path, energy_index)``; ``FILE_err.ext`` siblings supply
    relative errors when present. ``producer_version`` is required."""
def import_nifti(
    components: dict[
        str,
        str | PathLike[str] | tuple[str | PathLike[str], str | PathLike[str]],
    ],
    case_id: str,
    unit: str,
    normalization: str,
    producer_system: str,
    producer_version: str | None = None,
    frame_of_reference_uid: str | None = None,
) -> PhysicalDoseBundle:
    """Lift per-component NIfTI dose volumes into a physical dose bundle
    (same path as ``openbnct import nifti``). ``components`` maps each of
    ``boron``/``nitrogen``/``hydrogen``/``photon`` to a ``.nii``/``.nii.gz``
    path or a ``(path, sigma_path)`` tuple pairing the value volume with an
    absolute one-sigma volume on the same grid. ``producer_system`` is
    required because NIfTI headers carry no producer identity."""
def export_mcnp_deck(
    case: str | PathLike[str],
    output: str | PathLike[str],
    assignment: str | PathLike[str] | None = None,
    xs_suffix: str | None = None,
    seed: int | None = None,
) -> str:
    """Emit an MCNP input deck for a transport case (same path as ``openbnct
    export mcnp``) and write it to ``output``. ``xs_suffix`` (for example
    ``"80c"``) is the operator's declared cross-section library suffix;
    omitting it emits bare ZAIDs resolved by xsdir defaults. Returns the deck
    text."""
def import_external_dose(file: str | PathLike[str]) -> "ExternalDoseBundle":
    """Import a ``openbnct.external-dose/0.1.0`` document into a
    provenance-bound external dose bundle (same path as ``openbnct import
    dose``)."""
def load_external_dose_bundle(path: str | PathLike[str]) -> "ExternalDoseBundle":
    """Load an already-imported external dose bundle."""
def load_bed_bundle(path: str | PathLike[str]) -> "BedBundle":
    """Load an external BED/EQD2 bundle written by ``openbnct bio bed``."""
def bed_from_external_dose(
    dose: "ExternalDoseBundle",
    alpha_beta: float,
    region_alpha_beta: dict[str, float] | None = None,
    region_masks: list[tuple[str, str | PathLike[str]]] | None = None,
    quantity: str = "eqd2",
) -> "BedBundle":
    """Convert an external dose course to a BED or EQD2 field (same path as
    ``openbnct bio bed``)."""
def combine_biological_doses(
    primary: BiologicalDoseBundle,
    external: "BedBundle",
    resample: str | None = None,
    assumption: str = "",
) -> "CombinedDoseBundle":
    """Add an external EQD2 course to a photon-isoeffective BNCT EQD2 bundle
    (same path as ``openbnct bio combine``). ``assumption`` is a required
    operator statement recorded in the output."""

class ExternalDoseBundle:
    """A validated external-dose bundle (``openbnct.external-dose/0.1.0``)."""

    @property
    def schema_version(self) -> str: ...
    @property
    def case_id(self) -> str: ...
    @property
    def quantity(self) -> str:
        """``physical`` or ``rbe_weighted``."""
    @property
    def fractions(self) -> int: ...
    @property
    def values(self) -> list[float]: ...
    @property
    def absolute_standard_uncertainty(self) -> list[float] | None: ...
    @property
    def geometry(self) -> Geometry: ...
    @property
    def provenance_id(self) -> str: ...
    def to_json(self) -> str: ...
    def write(self, output: str | PathLike[str]) -> None: ...

class BedBundle:
    """A BED or EQD2 field derived from an external dose course."""

    @property
    def schema_version(self) -> str: ...
    @property
    def case_id(self) -> str: ...
    @property
    def quantity(self) -> str:
        """``bed`` or ``eqd2``."""
    @property
    def quantity_basis(self) -> str:
        """``physical`` or ``rbe_weighted`` basis of the source dose."""
    @property
    def alpha_beta(self) -> float: ...
    @property
    def fractions(self) -> int: ...
    @property
    def values(self) -> list[float]: ...
    @property
    def absolute_standard_uncertainty(self) -> list[float] | None: ...
    @property
    def geometry(self) -> Geometry: ...
    @property
    def external_dose_provenance(self) -> str: ...
    def to_json(self) -> str: ...
    def write(self, output: str | PathLike[str]) -> None: ...

class CombinedDoseBundle:
    """A combined BNCT + external-course biological evaluation (``eqd2``)."""

    @property
    def schema_version(self) -> str: ...
    @property
    def case_id(self) -> str: ...
    @property
    def quantity(self) -> str: ...
    @property
    def values(self) -> list[float]: ...
    @property
    def absolute_standard_uncertainty(self) -> list[float] | None: ...
    @property
    def inputs(self) -> list[tuple[str, str, str, str]]:
        """``(role, id, sha256, provenance_id)`` for each consumed input."""
    @property
    def external_resampling(self) -> str | None: ...
    @property
    def external_quantity_basis(self) -> str: ...
    @property
    def additivity_assumption(self) -> str: ...
    @property
    def qualification(self) -> str: ...
    def to_json(self) -> str: ...
    def write(self, output: str | PathLike[str]) -> None: ...

class DoseComparison:
    """A cross-code dose-comparison record (``openbnct.dose-comparison/0.1.0``)."""

    @property
    def schema_version(self) -> str: ...
    @property
    def case_id(self) -> str: ...
    @property
    def sigma_level(self) -> float: ...
    @property
    def voxel_count(self) -> int: ...
    @property
    def inputs(self) -> list[tuple[str, str, str, str]]:
        """``(role, id, sha256, provenance_id)`` for each compared input."""
    @property
    def quantities(self) -> list[tuple[str, str, float, float, float, float, float | None]]:
        """``(quantity, unit, max_abs, mean_abs, rms, max_normalized,
        within_sigma_fraction)`` per component plus ``physical_total``."""
    @property
    def qualification(self) -> str: ...
    def to_json(self) -> str: ...
    def write(self, output: str | PathLike[str]) -> None: ...
def compare_dose_bundles(
    reference: PhysicalDoseBundle,
    candidate: PhysicalDoseBundle,
    sigma_level: float = 2.0,
) -> DoseComparison:
    """Compare two physical dose bundles on the same frozen case (same path
    as ``openbnct compare``)."""

class GammaEvaluation:
    """A gamma-index evaluation record."""
    @property
    def schema_version(self) -> str: ...
    @property
    def case_id(self) -> str: ...
    @property
    def criteria(self) -> tuple[float, float, str, float | None]:
        """``(dose_difference_percent, distance_to_agreement_mm,
        normalization, dose_threshold_percent)``."""
    @property
    def inputs(self) -> list[tuple[str, str, str, str]]:
        """``(role, id, sha256, provenance_id)`` for each compared input."""
    @property
    def results(
        self,
    ) -> list[tuple[str, str, int, int, float, float | None, float | None, float | None]]:
        """``(quantity, unit, voxels_evaluated, voxels_excluded, pass_rate,
        mean_gamma, p95_gamma, max_gamma)`` per component plus
        ``physical_total``."""
    def gamma_volume(self, quantity: str) -> list[float | None] | None:
        """Per-voxel gamma for one quantity (grid order), when emitted;
        ``None`` entries mark threshold-excluded voxels."""
    @property
    def qualification(self) -> str: ...
    def to_json(self) -> str: ...
    def write(self, output: str | PathLike[str]) -> None: ...

def evaluate_gamma(
    reference: PhysicalDoseBundle,
    candidate: PhysicalDoseBundle,
    dose_difference_percent: float = 3.0,
    distance_to_agreement_mm: float = 3.0,
    normalization: str = "global",
    dose_threshold_percent: float | None = None,
    emit_gamma_volume: bool = False,
) -> GammaEvaluation:
    """Evaluate the Low gamma index between two physical dose bundles on
    the same frozen case (same path as ``openbnct gamma``)."""

class MultigroupData:
    """A validated multigroup cross-section artifact for the deterministic
    S_N solver."""
    @property
    def schema_version(self) -> str: ...
    @property
    def id(self) -> str: ...
    def to_json(self) -> str: ...

class MultigroupFlux:
    """A multigroup scalar-flux artifact produced by ``sn solve``."""
    @property
    def schema_version(self) -> str: ...
    @property
    def case_id(self) -> str: ...
    @property
    def beam_model(self) -> str:
        """``uncollided_split`` or ``boundary_flux``."""
    @property
    def quadrature_order(self) -> int: ...
    @property
    def outer_iterations(self) -> int: ...
    @property
    def residual(self) -> float: ...
    @property
    def converged(self) -> bool: ...
    @property
    def group_count(self) -> int: ...
    @property
    def voxel_count(self) -> int: ...
    def flux(self) -> list[list[float]]:
        """``[voxel][group]`` scalar flux, cm⁻²s⁻¹ per unit source rate."""
    def to_json(self) -> str: ...

class MultigroupCovariance:
    """A validated declared-uncertainty artifact over multigroup parameters."""
    @property
    def schema_version(self) -> str: ...
    @property
    def id(self) -> str: ...
    def to_json(self) -> str: ...

class DoseUncertaintyBudget:
    """A nuclear-data propagated dose-uncertainty budget artifact."""
    @property
    def schema_version(self) -> str: ...
    @property
    def id(self) -> str: ...
    def to_json(self) -> str: ...

class SensitivitySpec:
    """A validated declared-input sensitivity-screening specification."""
    @property
    def schema_version(self) -> str: ...
    @property
    def id(self) -> str: ...
    def to_json(self) -> str: ...

class SensitivityScreening:
    """A Morris/Sobol screening report artifact."""
    @property
    def schema_version(self) -> str: ...
    @property
    def id(self) -> str: ...
    def to_json(self) -> str: ...

class ResolvedWeightWindows:
    """A validated mesh weight-window artifact (CADIS, FW-CADIS, or manual)."""
    @property
    def schema_version(self) -> str: ...
    @property
    def id(self) -> str: ...
    def to_json(self) -> str: ...

class MeasurementRecord:
    """A validated published-measurement record with provenance."""
    @property
    def schema_version(self) -> str: ...
    @property
    def id(self) -> str: ...
    def to_json(self) -> str: ...

class MeasurementComparisonReport:
    """A computed-versus-measured comparison report artifact."""
    @property
    def schema_version(self) -> str: ...
    @property
    def id(self) -> str: ...
    def to_json(self) -> str: ...

class BeamDescription:
    """A validated beam-description contract (spectrum, angular, port)."""
    @property
    def schema_version(self) -> str: ...
    @property
    def id(self) -> str: ...
    def to_json(self) -> str: ...

class BeamQualityReport:
    """A beam-quality metrics report artifact."""
    @property
    def schema_version(self) -> str: ...
    @property
    def id(self) -> str: ...
    def to_json(self) -> str: ...

class AcceleratorSource:
    """A validated accelerator neutron-source contract."""
    @property
    def schema_version(self) -> str: ...
    @property
    def id(self) -> str: ...
    def to_json(self) -> str: ...

class BeamShapingAssembly:
    """A validated beam-shaping-assembly contract."""
    @property
    def schema_version(self) -> str: ...
    @property
    def id(self) -> str: ...
    def to_json(self) -> str: ...

class BsaSweepRecord:
    """A BSA parameter-sweep result record."""
    @property
    def schema_version(self) -> str: ...
    @property
    def id(self) -> str: ...
    def to_json(self) -> str: ...

class LinealSpectrum:
    """A validated lineal-energy spectrum artifact (MKM input)."""
    @property
    def schema_version(self) -> str: ...
    @property
    def id(self) -> str: ...
    def to_json(self) -> str: ...

class LinealTallySpec:
    """A validated lineal-tally specification for transport-derived spectra."""
    @property
    def schema_version(self) -> str: ...
    @property
    def id(self) -> str: ...
    def to_json(self) -> str: ...

class MetamorphicEvaluation:
    """A metamorphic-relation oracle evaluation report."""
    @property
    def schema_version(self) -> str: ...
    @property
    def id(self) -> str: ...
    def to_json(self) -> str: ...

class AnalyticOracle:
    """A validated analytic-transport oracle specification."""
    @property
    def schema_version(self) -> str: ...
    @property
    def id(self) -> str: ...
    def to_json(self) -> str: ...

class AnalyticOracleEvaluation:
    """An analytic-oracle evaluation report artifact."""
    @property
    def schema_version(self) -> str: ...
    @property
    def id(self) -> str: ...
    def to_json(self) -> str: ...

class BoronMicrodistribution:
    """A validated subcellular 10B microdistribution model."""
    @property
    def schema_version(self) -> str: ...
    @property
    def id(self) -> str: ...
    def to_json(self) -> str: ...

class MicrodistributionCorrection:
    """An evaluated microdistribution correction record."""
    @property
    def schema_version(self) -> str: ...
    @property
    def id(self) -> str: ...
    def to_json(self) -> str: ...

class RtPlanSummary:
    """An RTPLAN summary record produced by ``dicom rtplan-info`` or read
    back from an ``export-rtplan`` output."""
    @property
    def schema_version(self) -> str: ...
    @property
    def sop_instance_uid(self) -> str: ...
    @property
    def rt_plan_label(self) -> str: ...
    @property
    def rt_plan_name(self) -> str | None: ...
    @property
    def beam_count(self) -> int: ...
    def beams(self) -> list[tuple[int, str | None, float | None, list[float]]]:
        """``(beam_number, name, gantry_angle_deg, metersets)`` per beam;
        gantry angle comes from the first control point."""
    def to_json(self) -> str: ...

class ComponentNiftiManifest:
    """Manifest binding an exported per-component NIfTI set to its dose
    bundle."""
    @property
    def schema_version(self) -> str: ...
    @property
    def case_id(self) -> str: ...
    @property
    def unit(self) -> str: ...
    def files(
        self,
    ) -> list[tuple[str, str, str, str | None, str | None]]:
        """``(component, file, sha256, sigma_file, sigma_sha256)`` per
        exported NIfTI volume."""
    def to_json(self) -> str: ...

def load_multigroup_data(path: str | PathLike[str]) -> MultigroupData:
    """Read and validate a multigroup-data artifact."""

def load_multigroup_flux(path: str | PathLike[str]) -> MultigroupFlux:
    """Read a multigroup scalar-flux artifact produced by ``sn solve``."""

def load_multigroup_covariance(path: str | PathLike[str]) -> MultigroupCovariance:
    """Read and validate a multigroup-covariance artifact."""

def load_dose_uncertainty_budget(
    path: str | PathLike[str],
) -> DoseUncertaintyBudget:
    """Read and validate a dose-uncertainty-budget artifact."""

def load_sensitivity_spec(path: str | PathLike[str]) -> SensitivitySpec:
    """Read and validate a sensitivity-screening specification."""

def load_sensitivity_screening(
    path: str | PathLike[str],
) -> SensitivityScreening:
    """Read and validate a sensitivity-screening report."""

def load_weight_windows(path: str | PathLike[str]) -> ResolvedWeightWindows:
    """Read and validate a resolved weight-window artifact."""

def load_measurement_record(path: str | PathLike[str]) -> MeasurementRecord:
    """Read and validate a published-measurement record."""

def load_measurement_comparison(
    path: str | PathLike[str],
) -> MeasurementComparisonReport:
    """Read and validate a computed-versus-measured comparison report."""

def load_beam_description(path: str | PathLike[str]) -> BeamDescription:
    """Read and validate a beam-description contract."""

def load_beam_quality_report(path: str | PathLike[str]) -> BeamQualityReport:
    """Read and validate a beam-quality report."""

def load_accelerator_source(path: str | PathLike[str]) -> AcceleratorSource:
    """Read and validate an accelerator-source contract."""

def load_beam_shaping_assembly(
    path: str | PathLike[str],
) -> BeamShapingAssembly:
    """Read and validate a beam-shaping-assembly contract."""

def load_bsa_sweep(path: str | PathLike[str]) -> BsaSweepRecord:
    """Read and validate a BSA sweep record."""

def load_lineal_spectrum(path: str | PathLike[str]) -> LinealSpectrum:
    """Read and validate a lineal-energy spectrum artifact."""

def load_lineal_tally_spec(path: str | PathLike[str]) -> LinealTallySpec:
    """Read and validate a lineal-tally specification."""

def load_metamorphic_evaluation(
    path: str | PathLike[str],
) -> MetamorphicEvaluation:
    """Read and validate a metamorphic-evaluation report."""

def load_analytic_oracle(path: str | PathLike[str]) -> AnalyticOracle:
    """Read and validate an analytic-oracle specification."""

def load_analytic_oracle_evaluation(
    path: str | PathLike[str],
) -> AnalyticOracleEvaluation:
    """Read an analytic-oracle evaluation report."""

def load_gamma_evaluation(path: str | PathLike[str]) -> GammaEvaluation:
    """Read and validate a gamma-index evaluation report."""

def load_boron_microdistribution(
    path: str | PathLike[str],
) -> BoronMicrodistribution:
    """Read and validate a boron-microdistribution model."""

def load_microdistribution_correction(
    path: str | PathLike[str],
) -> MicrodistributionCorrection:
    """Read and validate a microdistribution-correction record."""

def load_rtplan_summary(path: str | PathLike[str]) -> RtPlanSummary:
    """Read an RTPLAN summary record."""

def summarize_rtplan(path: str | PathLike[str]) -> RtPlanSummary:
    """Summarize a DICOM RTPLAN file — the ``dicom rtplan-info`` surface."""

def load_component_nifti_manifest(
    path: str | PathLike[str],
) -> ComponentNiftiManifest:
    """Read a per-component NIfTI export manifest."""

def avify_export_plan(
    case: str | PathLike[str],
    assignment: str | PathLike[str],
    spec: str | PathLike[str],
    prefix: str | PathLike[str],
) -> str:
    """Export a case + material assignment + ``openbnct.avify-spec/0.1.0``
    document to the Avify Dose engine's voxel plan (same path as
    ``openbnct avify export-plan``). Returns a JSON object with the
    artifact paths, SHA-256 bindings, and per-class voxel counts. The
    declared uptake set passes through verbatim — corner maps stay in
    the separately licensed engine."""

def avify_verify(
    case: str | PathLike[str],
    assignment: str | PathLike[str],
    spec: str | PathLike[str],
    outdir: str | PathLike[str],
    engine_cmd: str = "avify-dose",
    threads: int | None = None,
    timeout_s: int = 21600,
) -> str:
    """Export, then run the separately licensed Avify Dose engine as a
    bounded child process (same path as ``openbnct avify verify`` —
    timeout kills and reaps the child). Writes ``certificate.json`` and
    the ``avify-run.json`` receipt under ``outdir``; returns a JSON
    object with the run record and the parsed certificate — an
    empirical two-evaluation envelope, research only."""

def avify_status(receipt: str | PathLike[str]) -> str:
    """Check an ``avify-run.json`` receipt against the filesystem (same
    path as ``openbnct avify status``). Returns a JSON object with
    per-input CURRENT/CHANGED/MISSING states and a ``stale`` verdict."""

def avify_load_certificate(certificate: str | PathLike[str]) -> str:
    """Read and validate the engine's ``certificate.json`` — returned
    verbatim as JSON; the binding never re-derives engine results."""

def avify_diff(
    before: str | PathLike[str], after: str | PathLike[str]
) -> str:
    """Compare two Avify run directories (or ``avify-run.json`` paths) —
    same path as ``openbnct avify diff``. Returns a JSON object with
    per-ROI certified-interval/action changes and which bound inputs
    differ."""
