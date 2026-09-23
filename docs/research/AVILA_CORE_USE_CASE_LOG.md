# Avila Core use-case record

This record tracks when OpenBNCT uses Avila Core, what Core contributed, and
what likely would have happened without it. It is an engineering record, not a
testimonial. Entries must keep direct observations separate from
counterfactual estimates.

## Recording rules

- Link the OpenBNCT and Core revisions, package, and durable run evidence.
- Record failures, neutral outcomes, and added work as well as benefits.
- Label a result **observed** only when repository history or run artifacts
  support it. Label controlled comparisons and timings **measured**.
- State counterfactuals as **inferred**, give a confidence level, and explain
  the basis. Do not turn an unmeasured estimate into hours saved.
- Say whether Core changed the domain result, the assurance around it, the
  development process, or some combination of those.
- Add a new dated entry for each bounded use rather than rewriting an earlier
  assessment with hindsight.

## Entry template

```text
Date and task:
OpenBNCT revision:
Core revision:
Core package/run evidence:

Starting point:
What Core did:
Observed result:
Development effect:
Likely result without Core (inferred):
Counterfactual confidence and basis:
What was not measured or established:
Follow-up:
```

## 2026-09-03 — JEFF-4.0 evidence-aware NJOY gate

**OpenBNCT revision:** `5b57f40` (`Dogfood Avila Core with the NJOY evidence
gate`)

**Core revisions exercised:** `1b2f3ff` (`Add actionable runtime diagnostics
and attempt logs`) and `cfe4eac` (`Add hash-bound external checker adapters`)

**Package and evidence:**
[`integrations/avila-core/njoy-evidence-aware/`](../../integrations/avila-core/njoy-evidence-aware/)
and [ADR 0024](../adr/0024-avila-core-evidence-loop.md)

### Starting point

OpenBNCT already owned the domain calculation and had a frozen six-document
evidence chain for the transported-photon KERMA assessment. It did not have a
narrow command for an external orchestrator to regenerate that assessment,
nor a Core package binding the exact inputs, checker, executable, output, and
requirements into one attempt record.

### What Core did

- Staged and hash-checked six OpenBNCT inputs.
- Bound the exact OpenBNCT executable and external-checker descriptor.
- Invoked OpenBNCT through a receipt-producing adapter and verified the output.
- Preserved the categorical scientific qualification separately from the
  numeric requirement.
- Evaluated the exact remaining-finding count against a declared limit.
- Recorded execution diagnostics and made replay inapplicable when free inputs
  changed, instead of comparing changed evidence with the frozen result.

### Observed result

- OpenBNCT regenerated and matched its evidence-aware result.
- The scientific result remained
  `transported_photon_kerma_rejected`, with `102` unexplained in-domain
  findings. Core correctly returned `FAIL` against a limit of zero while the
  checker process itself succeeded.
- The integration required OpenBNCT to expose the stable
  `njoy check-evidence-aware` machine boundary instead of asking Core to
  understand nuclear-data report internals.
- The exercise exposed that the checker descriptor's content identity was not
  initially part of Core's invocation and memoization identity. Core revision
  `cfe4eac` corrected that boundary.
- A changed-input run withheld frozen claims, reran the checker, and did not
  claim that replay of the reference case applied.

### Development effect

This first use was infrastructure investment, not a demonstrated time saving.
The OpenBNCT command and package were added while missing Core adapter behavior
was also being built. Core did improve consistency and inspectability in this
slice: the same attempt connected content identities, invocation, receipt,
claims, requirement verdict, and diagnostics without duplicating OpenBNCT's
scientific rules.

### Likely result without Core — inferred

OpenBNCT would very likely have reached the same scientific rejection and
count, because OpenBNCT computes and verifies those values. The probable
alternative was a direct CLI invocation or project-specific script plus manual
comparison of its JSON output. That would have been quicker for this one frozen
case, but it likely would not have produced the same reusable cross-project
package, verified attempt log, categorical/numeric claim separation, or
uniform changed-input behavior.

It is also plausible that the missing checker-descriptor identity would have
remained undiscovered until a later external-tool integration.

**Counterfactual confidence:**

- **High** that the scientific result would be unchanged: it is generated and
  independently checked inside OpenBNCT.
- **Medium** that the fallback would have been a bespoke command/script and
  manual evidence review: that matches the pre-integration boundary, but no
  controlled implementation was built for comparison.
- **Medium** that the descriptor-identity defect would have survived longer:
  this use exposed it, but another integration could also have done so.
- **Low / not estimated** for calendar or engineering time saved. No comparable
  baseline or prospective timing was captured, and this slice expanded Core
  itself.

### What this entry does not establish

It does not validate the scientific method, qualify a response table, show a
net speed improvement, or demonstrate reuse across a second OpenBNCT task. A
credible productivity claim needs prospective task timing or comparable work,
and a credible consistency claim needs the same Core path reused on later
investigations.

### Follow-up

For the next bounded OpenBNCT task, record the starting state and start time
before using Core, then capture interventions, failed attempts, output changes,
and elapsed active work. Reuse this package shape where it fits rather than
expanding Core merely to make the record look favorable.

## 2026-09-03 — Triage the 102 remaining NJOY findings

**Starting OpenBNCT revision:** `e561226`

**Implemented OpenBNCT revision:** `0629ba8` (`Triage the remaining NJOY
diagnostics`)

**Core revision exercised:** `51e2b59` (`Add closed-set categorical
requirements`)

**Package and evidence:**
[`integrations/avila-core/njoy-evidence-aware/`](../../integrations/avila-core/njoy-evidence-aware/)
and [ADR 0025](../adr/0025-diagnostic-triage-of-remaining-njoy-findings.md)

### Starting point

The frozen Core gate reproduced one exact result: 102 in-domain findings
against a limit of zero. The category
`transported_photon_kerma_rejected` was preserved in claims but could not be a
requirement. The 102 findings were already attributable by nuclide—C-13 (32),
O-17 (43), and O-18 (27)—but the active investigation queue did not distinguish
runs blocked by missing photon-production sources from runs needing an
independent reaction diagnostic.

A prospective baseline was captured before changing the slice:

| Observation | Result |
| --- | ---: |
| OpenBNCT debug build wall time | 0.71 s |
| Original Core checker execution stage | 20 ms |
| Original cold `cargo run` wall time | 5.74 s |
| Original technical result | `FAIL`, 102 against 0 |

The cold Core wall time includes compiling Core and is not comparable to a
warm deployed runner. It is retained as setup evidence, not a speed claim.

### What Core did

- Reused the existing package boundary to bind seven inputs, the OpenBNCT
  executable, the checker descriptor, and one deterministic output.
- Exposed a concrete compiler gap: a categorical scientific rejection could
  be transported but not required.
- Added a general, role-owned closed categorical vocabulary and explicit
  `equals` / `in_set` requirements in Core rather than an OpenBNCT-specific
  exception.
- Independently evaluated the exact diagnostic queue and response category.
- Re-hashed 4 package documents and 8 artifacts, staged 7 inputs, verified the
  execution receipt and reproduced output, generated 3 typed claims, admitted
  all 10 evidence records, and compared generated claims with the committed
  claims document.

### Observed result

- OpenBNCT preserved all 102 findings and partitioned them into 59 findings on
  source-data-blocked C-13/O-18 runs and 43 O-17 findings requiring an
  independent reaction diagnostic.
- The response qualification did not change:
  `transported_photon_kerma_rejected`.
- A fresh complete Core run reproduced the expected output and returned two
  separate verdicts:
  `FAIL / bounded.le.exceeds` for 43 against 0, and
  `FAIL / categorical.equals.mismatch` for rejected versus
  candidate-unreviewed.
- The final warm end-to-end invocation took 0.44 s wall time; the checker stage
  took 40 ms. These are single observations, not a benchmark distribution.
- One setup invocation used the pre-rebuild OpenBNCT binary and rejected the
  new subcommand as unknown. Rebuilding fixed it. Core did not prevent that
  retry; the final executable hash pin prevents that stale binary from entering
  the recorded run.

The deterministic identities were:

- triage report:
  `6ba92bce735cf290dd3dbe3e068ceff1e25cbc1869b21d5ecd64db8b8d206020`;
- checker result:
  `aff141e8786f8c7cd4729e6a0a7f29ecb5c0db5bdabab7636d633db789b3cdf0`;
- compiled Core snapshot:
  `7c44c22e634c6e65349ba075c97c206a838b7a7040b25a9f7954e102235624a3`;
- campaign:
  `5feff819d1abe6abeb6ffdd9e691bc3193f9cc6d45be690267a2133f97a7c417`.

### Development effect

The scientific partition is OpenBNCT's result, not Core's. Core's direct value
was control and feedback: it made the count/category distinction executable,
made stale or mismatched bytes fail closed, and replaced separate manual hash,
staging, extraction, expected-output, admission, and verdict checks with one
repeatable path.

This pass was still infrastructure investment. Supporting the real use case
required Core commit `51e2b59`, which changed 31 files (+1,113/-83). It was
almost certainly slower than writing a one-off script for this single frozen
case. The reusable value must be judged on later slices that consume the
categorical facility without extending Core again.

### Likely result without Core — inferred

OpenBNCT would probably have reached the same 59/43 partition and unchanged
scientific rejection, because those are derived and verified inside OpenBNCT.
The likely fallback is a direct `openbnct` invocation followed by a JSON diff
or small project-specific script. That would be adequate for the immediate
physics result.

Without Core, it is less likely that both the numeric queue and response
category would have been encoded as independent, reusable requirements in the
same attempt record. The category would probably have remained a human-read
field while the count alone drove a scripted gate. The exact seven-input
lineage, executable identity, generated-claim comparison, and uniform failure
surface would also have required bespoke work or manual checking.

**Counterfactual confidence:**

- **High** that the 59/43 partition and rejection would be unchanged.
- **Medium** that the fallback would check the count automatically but inspect
  the category manually; no parallel implementation was built.
- **High** that a one-off path would have been faster for this single pass,
  because Core itself needed a material categorical-requirement extension.
- **Not estimated** for cumulative time saved. Reuse has not yet been measured.

### What this entry does not establish

It does not explain O-17's 43 findings, solve C-13/O-18's absent photon source,
qualify a response treatment, or prove a net productivity gain. The two wall
times are different cache states and must not be treated as an A/B comparison.

### Follow-up

Use the unchanged Core path for the O-17 reaction diagnostic and record how
many attempts, compiler/runtime interventions, and manual control steps it
requires. That is the first opportunity to observe reuse rather than Core
feature construction.

## 2026-09-03 — O-17 processor energy-balance attribution

**Starting OpenBNCT revision:** `c52a31c`

**Implemented OpenBNCT revision:** `5729f8a` (`Attribute the O17 NJOY
energy-balance findings`)

**Core revision exercised:** `2e58d1d` (a Core commit that gates another
integration's route readiness); no Core source change was made for this
OpenBNCT slice

**Package and evidence:**
[`integrations/avila-core/njoy-evidence-aware/`](../../integrations/avila-core/njoy-evidence-aware/),
[ADR 0026](../adr/0026-o17-processor-energy-balance-attribution.md), and the
ignored local workspace `runs/avila-core-o17-regression/`

### Starting point

The previous triage left exactly 43 O-17 findings in the independent reaction
queue. A prospective start was recorded at `2026-09-03T23:30:51-04:00`, along
with the starting revision, the O-17 processor-output SHA-256
`e7ec927a4d7faeb49a2f6c7cc89c0a22c322818252763899b950e41d39f38e61`,
and a targeted 50-test baseline that completed in 15.22 seconds.

The intended slice was bounded: identify whether those findings have a
deterministic reaction-level cause, preserve any distinction between processor
attribution and physical validation, then run the existing Core gate without
adding a Core feature.

### What Core did

- Reused the same contract, registry, adapter, expected output, requirements,
  and compiled snapshot as the diagnostic-triage pass.
- Required a deliberate executable and claims-producer re-pin after the
  OpenBNCT binary changed; no hash bypass was used.
- Re-hashed 4 package documents and 8 artifacts, staged 7 inputs, verified 10
  evidence records, executed the unchanged check in 40 ms, and reproduced the
  committed output SHA-256
  `aff141e8786f8c7cd4729e6a0a7f29ecb5c0db5bdabab7636d633db789b3cdf0`.
- Preserved separate numeric and categorical failures after the new diagnostic
  was added to OpenBNCT.

### Observed result

OpenBNCT added a receipt-bound processor-accounting report. For all 43
in-domain O-17 findings, the summed printed File 6 `ebal` contributions match
the final `MT301 - kinematic maximum` excess within the declared `2e-3` print
tolerance. The maximum observed relative difference is
`3.721276397804447e-4`; MT 443 matches the kinematic maximum at every sample.
The report preserves 43 physical validations required and zero waivers.

The generic MF=6/MT=102 capture calculator was tried first and returned
`invalid ENDF tabulation`. Inspection showed that O-17's capture source and
other contributing reactions exceed that calculator's narrow supported shape.
This was a useful rejected attempt, but its error text did not identify the
unsupported representation; source and processor-output inspection supplied
that diagnosis.

The fresh Core run required no compiler/runtime intervention and no Core code
change. It completed in 0.49 seconds wall time and correctly returned:

- `FAIL / bounded.le.exceeds` for 43 against 0; and
- `FAIL / categorical.equals.mismatch` for
  `transported_photon_kerma_rejected` versus
  `transported_photon_kerma_candidate_unreviewed`.

The Core manifest SHA-256 is
`4bbd8fb986cf4346f8cfcdfa6035808b9846b7a0cc3febe322a47c3eb509863d`,
the unchanged compiled snapshot is
`7c44c22e634c6e65349ba075c97c206a838b7a7040b25a9f7954e102235624a3`,
the execution receipt is
`d8a291d4296e6ab8556bc51eed5436990a418d8eaf8136b11dfe75fff6dd2fcd`,
and the campaign is
`3ff49adb71b537dc4ebee3f2d8710431a2394bd60cc195692fb725a56b8b316f`.

### Development effect

This is the first OpenBNCT reuse that required no Core capability or semantic
change. Core did not discover the O-17 mechanism; OpenBNCT did. Core's value
was making the non-effect on readiness executable: adding an explanatory
artifact could not silently turn processor attribution into a passing physical
gate. The integration work was limited to re-pinning the rebuilt executable in
the package and its committed producer identities.

The OpenBNCT verifier independently regenerated the 48,149-byte attribution
and checked the entire external execution root in 1.53 seconds. These command
timings are single observations, not benchmark distributions. No reliable
active-development duration or controlled A/B implementation was measured.

### Likely result without Core — inferred

The processor mechanism and scientific pause decision would almost certainly
be the same; they come from OpenBNCT's parser, the pinned NJOY source, and the
frozen output. A likely non-Core workflow would rerun the OpenBNCT verifier,
inspect that the old triage JSON had not changed, and record the conclusion in
the ADR. That would be adequate and slightly less package maintenance for this
one slice.

Without Core, the unchanged count and rejected category would be more likely
to remain a human regression check. Core supplied a uniform proof that the
same executable still reproduced the complete evidence chain and that both
requirements remained failed after an attractive mechanistic explanation was
introduced.

**Counterfactual confidence:**

- **High** that the O-17 accounting result and pause decision would be
  unchanged.
- **High** that the direct OpenBNCT verifier would be the fallback.
- **Medium** that the response category would otherwise be checked manually;
  a small project-specific regression script was also plausible.
- **Not estimated** for time saved. This pass demonstrates behavioral reuse and
  consistency, not a measured productivity advantage.

### What this entry does not establish

It does not independently validate O-17 reaction physics, reduce the triage
queue, qualify a response table, solve C-13/O-18's absent photon sources, or
reverse N-15's independent capture-balance rejection. It also does not show
that Core accelerated the scientific diagnosis.

### Follow-up

Pause this response-qualification path until a controlled alternative data
profile or independently reviewed O-17 physical calculation is available. The
next Core use should come from a genuinely needed project slice rather than
extending either repository solely to manufacture reuse evidence.
