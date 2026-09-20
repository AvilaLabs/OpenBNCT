// SPDX-License-Identifier: Apache-2.0

//! Minimal ENDF-6 MF33 (cross-section covariance) reader feeding the
//! `openbnct.multigroup-covariance` contract — the nuclear-data
//! uncertainty source for `uq::propagate_uncertainty`.
//!
//! Scope is deliberately narrow and honest: NI-type sub-subsections
//! with LB ∈ {0, 1, 5} — diagonal (absolute or relative) and symmetric
//! energy-grid covariance matrices. NC-type parameter covariances and
//! LB ∈ {2,3,4,6} are refused rather than misread; a tape declaring
//! them reports exactly which features were skipped.

use crate::endf_mf3::{endf_float, parse_record};

/// One parsed NI sub-subsection: a relative (or absolute) covariance
/// matrix over `intervals` = `energies_ev.len() − 1` energy bins.
#[derive(Debug, Clone, PartialEq)]
pub struct Mf33Covariance {
    /// The reaction this covariance describes (subsection's MT1 —
    /// the MT of the file section is the *target* reaction; MF33
    /// sections in neutron tapes normally carry MT of the same file's
    /// reaction set, subsections pair it via MAT1/MT1).
    pub reaction_mt: i32,
    /// LB flag as declared: 0 absolute diagonal, 1 relative diagonal,
    /// 5 relative symmetric matrix.
    pub lb: u8,
    /// Energy grid boundaries, ascending eV — covariance bins are the
    /// `len-1` intervals.
    pub energies_ev: Vec<f64>,
    /// Covariance values: `intervals` diagonal entries for LB 0/1, or
    /// `intervals×intervals` row-major for LB 5.
    pub matrix: Vec<f64>,
    /// True when LB=0 — values are absolute covariances (barns²), not
    /// relative. Collapse converts via the group mean cross section.
    pub absolute: bool,
}

/// What a tape's MF33 contributed: parsed sub-subsections plus an
/// explicit skip ledger for features outside scope.
#[derive(Debug, Clone, Default)]
pub struct Mf33Tape {
    pub covariances: Vec<Mf33Covariance>,
    /// Human-readable skips — NC parameter covariances, unsupported LB.
    pub skipped: Vec<String>,
}

/// Parse a tape's MF33 sections. `mt_filter` selects which target
/// reaction's section to read (`None` reads all).
pub fn parse_mf33(text: &str, mt_filter: Option<i32>) -> Result<Mf33Tape, String> {
    let lines: Vec<&str> = text.lines().collect();
    let mut tape = Mf33Tape::default();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        i += 1;
        if line.len() < 75 {
            continue;
        }
        let head = parse_record(line);
        if head.mf != 33 || head.mt == 0 {
            continue;
        }
        if let Some(want) = mt_filter
            && head.mt != want
        {
            continue;
        }
        // MF33 section HEAD: N2 = total subsection count (NC + NI).
        let subsections = head.n2.max(0) as usize;
        for _ in 0..subsections {
            let Some(sub) = lines.get(i) else { break };
            i += 1;
            let sub_head = parse_record(sub);
            let (nc, ni) = (sub_head.n1.max(0) as usize, sub_head.n2.max(0) as usize);
            // NC sub-subsections are LIST records we do not interpret —
            // skip their data lines honestly (N1 entries each).
            for _ in 0..nc {
                let Some(nc_head) = lines.get(i) else { break };
                let r = parse_record(nc_head);
                i += 1 + (r.n1.max(0) as usize).div_ceil(6);
            }
            if nc > 0 {
                tape.skipped.push(format!(
                    "MT{} subsection MAT1={} MT1={}: {nc} NC-type entries skipped",
                    head.mt, sub_head.l1, sub_head.l2
                ));
            }
            for _ in 0..ni {
                let Some(ni_line) = lines.get(i) else { break };
                let ni_head = parse_record(ni_line);
                i += 1;
                let (lb, nt, ne) = (
                    ni_head.l2 as u8,
                    ni_head.n1.max(0) as usize,
                    ni_head.n2.max(0) as usize,
                );
                let list_lines = nt.div_ceil(6);
                let mut numbers = Vec::with_capacity(nt);
                for _ in 0..list_lines {
                    let Some(data) = lines.get(i) else { break };
                    i += 1;
                    for a in (0..60).step_by(11) {
                        if numbers.len() < nt {
                            numbers.push(endf_float(data.get(a..a + 11).unwrap_or("")));
                        }
                    }
                }
                let energies: Vec<f64> = numbers.iter().take(ne).copied().collect();
                let f: Vec<f64> = numbers.iter().skip(ne).copied().collect();
                let intervals = ne.saturating_sub(1);
                let supported = match lb {
                    0 | 1 => f.len() >= intervals,
                    // Triangular rows: intervals + (intervals−1) + … + 1.
                    5 => f.len() >= intervals * (intervals + 1) / 2,
                    _ => false,
                };
                if !supported || intervals == 0 {
                    tape.skipped.push(format!(
                        "MT{} NI sub-sub LB={lb} NE={ne}: unsupported or malformed",
                        head.mt
                    ));
                    continue;
                }
                let matrix = match lb {
                    0 | 1 => f[..intervals].to_vec(),
                    _ => {
                        // Triangular rows: row r carries intervals−r
                        // entries; expand to row-major symmetric.
                        let mut m = vec![0.0; intervals * intervals];
                        let mut cursor = 0;
                        for row in 0..intervals {
                            for col in row..intervals {
                                if let Some(v) = f.get(cursor) {
                                    m[row * intervals + col] = *v;
                                    m[col * intervals + row] = *v;
                                }
                                cursor += 1;
                            }
                        }
                        m
                    }
                };
                // MT1=0 means "same reaction as the section".
                let reaction_mt = if sub_head.l2 == 0 {
                    head.mt
                } else {
                    sub_head.l2 as i32
                };
                tape.covariances.push(Mf33Covariance {
                    reaction_mt,
                    lb,
                    energies_ev: energies,
                    matrix,
                    absolute: lb == 0,
                });
            }
        }
    }
    Ok(tape)
}

/// Collapse an energy-grid covariance onto multigroup boundaries by
/// overlap-weighted averaging — `cov[g][h] = Σ_ij cov_ij · w_i(g)·w_j(h)
/// / (Δg·Δh)` where `w_i(g)` is the shared energy width of covariance
/// interval i and group g. First-order energy collapse; documented as
/// such on the artifact.
pub fn collapse_covariance_to_groups(
    cov: &Mf33Covariance,
    boundaries_ev: &[f64],
) -> Option<Vec<f64>> {
    let groups = boundaries_ev.len().checked_sub(1)?;
    let intervals = cov.energies_ev.len().checked_sub(1)?;
    if groups == 0 || intervals == 0 {
        return None;
    }
    let mut out = vec![0.0; groups * groups];
    for g in 0..groups {
        let (lo_g, hi_g) = (
            boundaries_ev[g].min(boundaries_ev[g + 1]),
            boundaries_ev[g].max(boundaries_ev[g + 1]),
        );
        let width_g = hi_g - lo_g;
        if width_g <= 0.0 {
            continue;
        }
        for h in 0..groups {
            let (lo_h, hi_h) = (
                boundaries_ev[h].min(boundaries_ev[h + 1]),
                boundaries_ev[h].max(boundaries_ev[h + 1]),
            );
            let width_h = hi_h - lo_h;
            if width_h <= 0.0 {
                continue;
            }
            let mut acc = 0.0;
            for i in 0..intervals {
                let wi = (hi_g.min(cov.energies_ev[i + 1]) - lo_g.max(cov.energies_ev[i])).max(0.0);
                if wi <= 0.0 {
                    continue;
                }
                for j in 0..intervals {
                    let wj =
                        (hi_h.min(cov.energies_ev[j + 1]) - lo_h.max(cov.energies_ev[j])).max(0.0);
                    if wj <= 0.0 {
                        continue;
                    }
                    let c = if intervals == 1 || cov.matrix.len() == intervals {
                        // diagonal-only storage
                        if i == j { cov.matrix[i] } else { 0.0 }
                    } else {
                        cov.matrix[i * intervals + j]
                    };
                    acc += c * wi * wj;
                }
            }
            out[g * groups + h] = acc / (width_g * width_h);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CONT record: fields 0-1 reals, 2-5 integers — ENDF control
    /// fields are integers, not E-format reals.
    fn fmt_row(fields: [f64; 6], mf: i32, mt: i32) -> String {
        format!(
            "{:>11}{:>11}{:>11}{:>11}{:>11}{:>11} 999{:>2}{:>3}    1",
            fmt_e(fields[0]),
            fmt_e(fields[1]),
            fields[2] as i64,
            fields[3] as i64,
            fields[4] as i64,
            fields[5] as i64,
            mf,
            mt
        )
    }

    /// LIST data line: all six fields are reals.
    fn fmt_list(fields: [f64; 6], mf: i32, mt: i32) -> String {
        format!(
            "{:>11}{:>11}{:>11}{:>11}{:>11}{:>11} 999{:>2}{:>3}    1",
            fmt_e(fields[0]),
            fmt_e(fields[1]),
            fmt_e(fields[2]),
            fmt_e(fields[3]),
            fmt_e(fields[4]),
            fmt_e(fields[5]),
            mf,
            mt
        )
    }
    fn fmt_e(v: f64) -> String {
        format!("{:.6E}", v)
            .replace("E+0", "+")
            .replace("E-0", "-")
            .replace("E+", "+")
            .replace("E-", "-")
    }

    #[test]
    fn parses_lb5_symmetric_block() {
        // Section MT=1, one subsection MAT1=0/MT1=1, one NI LB=5,
        // NE=3 energies → 2×2 covariance, NT = 3 + 3 (triangular).
        let mut text = String::new();
        text.push_str(&fmt_row([5.0e3, 10.0, 0.0, 0.0, 0.0, 1.0], 33, 1));
        text.push('\n');
        text.push_str(&fmt_row([0.0, 0.0, 0.0, 1.0, 0.0, 1.0], 33, 1));
        text.push('\n');
        text.push_str(&fmt_row([0.0, 0.0, 1.0, 5.0, 6.0, 3.0], 33, 1));
        text.push('\n');
        // LIST: E1,E2,E3 then triangular F: F11, F12, F22.
        text.push_str(&fmt_list([1e-5, 1e6, 2e7, 0.01, 0.002, 0.04], 33, 1));
        text.push('\n');
        let tape = parse_mf33(&text, Some(1)).unwrap();
        eprintln!(
            "covariances={:?} skipped={:?} text={:?}",
            tape.covariances, tape.skipped, text
        );
        assert_eq!(tape.covariances.len(), 1);
        let cov = &tape.covariances[0];
        assert_eq!(cov.lb, 5);
        assert_eq!(cov.energies_ev, vec![1e-5, 1e6, 2e7]);
        // Symmetric 2×2: [0.01, 0.002; 0.002, 0.04]
        assert_eq!(cov.matrix, vec![0.01, 0.002, 0.002, 0.04]);
    }

    #[test]
    fn real_b10_tape_parses() {
        let path = std::path::Path::new(
            "/home/connoravila/Documents/Avila-Labs/.nctforge-data/nctforge/endfb81-sources/nf-bnct-001/n-005_B_010.endf",
        );
        if !path.exists() {
            return; // dev-machine tape absent on CI
        }
        let text = std::fs::read_to_string(path).unwrap();
        let tape = parse_mf33(&text, None).unwrap();
        assert!(!tape.covariances.is_empty());
        for cov in &tape.covariances {
            assert!(cov.energies_ev.len() >= 2);
            assert!(cov.matrix.iter().all(|v| v.is_finite()));
            eprintln!(
                "MT{} LB={} intervals={} | first diag {}",
                cov.reaction_mt,
                cov.lb,
                cov.energies_ev.len() - 1,
                cov.matrix.first().copied().unwrap_or(0.0)
            );
        }
    }

    #[test]
    fn collapses_to_group_grid() {
        let cov = Mf33Covariance {
            reaction_mt: 1,
            lb: 5,
            energies_ev: vec![1.0, 10.0, 100.0],
            matrix: vec![0.01, 0.002, 0.002, 0.04],
            absolute: false,
        };
        // Two groups exactly matching the covariance intervals.
        let g = collapse_covariance_to_groups(&cov, &[1.0, 10.0, 100.0]).unwrap();
        assert_eq!(g.len(), 4);
        assert!((g[0] - 0.01).abs() < 1e-9);
        assert!((g[3] - 0.04).abs() < 1e-9);
    }
}
