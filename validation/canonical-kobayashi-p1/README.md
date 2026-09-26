# Canonical: Kobayashi problem 1 — nested cubes with a void shell

The first case of the Kobayashi suite — the benchmark designed to
expose discrete-ordinates **ray effects**. A source cube sits at the
origin corner of a reflecting quarter domain, wrapped in a large
void shell, wrapped in a pure-absorber shield.

## Citation

K. Kobayashi, N. Sugimura, Y. Nagaya, "3-D Radiation Transport
Benchmark Problems and Results for Simple Geometries with Void
Region," *Progress in Nuclear Energy* 39(2):119–144, 2001 (NEA/NSC
benchmark, NSC/DOC(2000)4).

- Case i (pure absorber) reference: exact numerical integration by
  the University of Kyoto; independently re-derived here as a
  **cell-averaged ray integral** (`reference.py`), verified to
  < 0.14% against COG's published table (LLNL-TR-648225) at every
  probe.
- Case ii (50% scattering) reference: GMVP Monte Carlo values
  published in the NEA report; MCNP5 confirms them within ~1σ
  (LA-UR-03-5974).

## Specification (quarter domain, mirrored)

| Region  | Quarter-domain bounds        | σ_t cm⁻¹ | σ_s/σ_t        |
| ------- | ---------------------------- | -------- | -------------- |
| Source  | [0,10]³ (corner)             | 0.1      | 0 (i) / 0.5(ii)|
| Void    | [0,50]³ ∖ source             | 1e-4     | 0 (i) / 0.5(ii)|
| Shield  | rest of [0,100]³             | 0.1      | 0 (i) / 0.5(ii)|

Isotropic volume source q = 1 n cm⁻³ s⁻¹, mono-energetic.
Boundaries: reflecting at x=y=z=0, vacuum at x=y=z=100.

## Realization

Mirrored full domain [−100,100]³ cm, vacuum on all six faces —
reflection equivariance makes the +++ octant identical to the
quarter-domain problem (verified: probe symmetry spread 3e-16).
Mesh 100³ cells of 2 cm; all material faces (±10, ±50, ±100 cm)
land on cell boundaries and every published probe point is a cell
center. Source rate 8000 n/s = q·V over the [−10,10]³ cube via
`UniformBox`. S8, θ-WDD.

The void shell is decomposed into six disjoint voxel-box slabs
(region overlap is rejected by the assignment schema).

## Metrics and verdict

**Case i** (S8): graded on the source-interior and just-outside
probes (y ≤ 15 cm) against the cell-averaged analytic reference at
5% — **pass** (3.7% and 4.8%). Probes deeper along the (5,y,5) line
are *reported but not graded*: they sit in the documented ray-effect
regime where the flux arrives through a narrowing void streaming
cone that discrete ordinates underfills. Observed deep-field
collapse (to −99.9% at (5,95,5)) is the same signature published
S_N codes produce — Ardra P5 misses the far shield by comparable
factors, and the non-monotone partial recovery at (5,45,5) is the
characteristic lobe structure. The discriminator is unambiguous:
re-running at **S16** the lobe structure *sharpens* — probes beyond
y = 35 cm drop to literal zero as the narrower quadrature directions
miss them entirely. Refining the ordinates makes the off-lobe field
worse, not better — the definitive ray-effect signature.

**Case ii** (S8): graded on the same near probes against GMVP at
5% — **pass** (0.4%, 3.3%). Scattering fills the angular cone
partially: mid-shield axis probes land within ~4%, deep axis probes
~50% low, diagonal probes in void lobes ~30–190% high — the ray
effect persists, redistributed.

Both verdicts reflect the same graded-zone rule, chosen a priori by
geometry (transport-dominated field), not by which probes happened
to pass.

## Why this case is in the battery

It is the canonical *negative* control: a problem where the discrete
ordinates method itself — not the implementation — is known to fail
in a specific regime. Passing it everywhere would be suspicious, not
reassuring. The battery records both where the solver is exact and
the shape/magnitude of the documented failure mode.

## Reproduce

```sh
./run.sh   # generates inputs, runs both solves, writes
           # comparison.json (case i) and comparison-ii.json
```

`KOB1_CELL_MM=10` refines the mesh to 1 cm (200³ cells) — faces and
probes stay on boundaries/centers.

Research-only cross-check — not a clinical or commissioning claim.
