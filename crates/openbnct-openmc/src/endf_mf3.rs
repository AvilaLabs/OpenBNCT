// SPDX-License-Identifier: Apache-2.0

//! Minimal ENDF-6 fixed-format reader for incident-neutron MF3
//! pointwise cross sections, plus the MF1 AWR mass.
//!
//! Scope: TEXT/HEAD/TAB1/INTTAB/SEND record structure, interpolation
//! laws 1 (histogram), 2 (linear-linear), 3 (lin-log), 4 (log-lin) and
//! 5 (log-log). Reads raw evaluation tapes and NJOY-produced PENDF
//! (identical record structure) — which is the point: `sn collapse`
//! can consume a NJOY-broadened tape without an OpenMC-HDF5
//! intermediate.

use std::collections::BTreeMap;

/// One parsed fixed-format record.
struct Record {
    c1: f64,
    c2: f64,
    l1: i64,
    l2: i64,
    n1: i64,
    n2: i64,
    mf: i32,
    mt: i32,
}

/// ENDF FORTRAN real: fields may drop the `E` (" 1.234567-03") or use
/// `D`/`d` exponents.
fn endf_float(s: &str) -> f64 {
    let t = s.trim();
    if t.is_empty() {
        return 0.0;
    }
    let t = t.replace(['D', 'd'], "e");
    if t.contains('e') || t.contains('E') {
        return t.parse().unwrap_or(0.0);
    }
    // No exponent letter: split at the last + or − after position 0.
    if let Some(pos) = t
        .rmatch_indices(['+', '-'])
        .map(|(i, _)| i)
        .find(|&i| i > 0)
    {
        let (mantissa, exp) = t.split_at(pos);
        return format!("{mantissa}e{exp}").parse().unwrap_or(0.0);
    }
    t.parse().unwrap_or(0.0)
}

fn endf_int(s: &str) -> i64 {
    s.trim().parse().unwrap_or(0)
}

fn parse_record(line: &str) -> Record {
    let f = |a: usize, b: usize| endf_float(line.get(a..b).unwrap_or(""));
    let i = |a: usize, b: usize| endf_int(line.get(a..b).unwrap_or(""));
    Record {
        c1: f(0, 11),
        c2: f(11, 22),
        l1: i(22, 33),
        l2: i(33, 44),
        n1: i(44, 55),
        n2: i(55, 66),
        mf: i(70, 72) as i32,
        mt: i(72, 75) as i32,
    }
}

/// One MF3 section: reaction MT, Q value (eV), and the TAB1 pointwise
/// table with its interpolation regions.
#[derive(Debug, Clone)]
pub struct EndfXsSection {
    pub q_ev: f64,
    /// (NBT, INT) region table — region boundaries and laws.
    pub regions: Vec<(u32, u32)>,
    pub energy_ev: Vec<f64>,
    pub xs_barns: Vec<f64>,
}

impl EndfXsSection {
    /// Evaluate σ(E) honoring the section's declared interpolation
    /// law; zero outside the tabulated range.
    pub fn at(&self, e: f64) -> f64 {
        let xs_e = &self.energy_ev;
        if e < xs_e[0] || e > *xs_e.last().unwrap_or(&0.0) {
            return 0.0;
        }
        let i = xs_e.partition_point(|&x| x <= e).clamp(1, xs_e.len() - 1);
        let (e0, e1) = (xs_e[i - 1], xs_e[i]);
        let (s0, s1) = (self.xs_barns[i - 1], self.xs_barns[i]);
        if e == e0 || e1 <= e0 {
            return s0;
        }
        // Interpolation law for the segment ending at point i.
        let law = self
            .regions
            .iter()
            .find(|(nbt, _)| i as u32 <= *nbt)
            .map(|(_, int)| *int)
            .unwrap_or(2);
        match law {
            1 => s0,
            3 => {
                // y linear in ln E
                let t = (e.ln() - e0.ln()) / (e1.ln() - e0.ln());
                s0 + (s1 - s0) * t
            }
            4 => {
                // ln y linear in E
                if s0 > 0.0 && s1 > 0.0 {
                    let t = (e - e0) / (e1 - e0);
                    (s0.ln() + t * (s1.ln() - s0.ln())).exp()
                } else {
                    s0 + (s1 - s0) * (e - e0) / (e1 - e0)
                }
            }
            5 => {
                // log-log
                if s0 > 0.0 && s1 > 0.0 && e0 > 0.0 && e1 > 0.0 {
                    let t = (e.ln() - e0.ln()) / (e1.ln() - e0.ln());
                    (s0.ln() + t * (s1.ln() - s0.ln())).exp()
                } else {
                    s0 + (s1 - s0) * (e - e0) / (e1 - e0)
                }
            }
            _ => s0 + (s1 - s0) * (e - e0) / (e1 - e0),
        }
    }
}

/// Parsed contents of one tape: MF3 sections by MT and the MF1 AWR
/// (mass relative to the neutron).
pub struct EndfTape {
    pub awr: f64,
    pub sections: BTreeMap<u32, EndfXsSection>,
}

/// Parse an ENDF-6 tape. `Err` on structural failure; unknown record
/// kinds are skipped so partial MF3 coverage still loads.
pub fn parse_endf(text: &str) -> Result<EndfTape, String> {
    let mut awr = 0.0;
    let mut sections = BTreeMap::new();
    let mut lines = text.lines().peekable();

    while let Some(line) = lines.next() {
        if line.len() < 75 {
            continue;
        }
        let head = parse_record(line);
        if head.mf == 1 && head.mt == 451 && awr == 0.0 {
            awr = head.c2; // MF1/451 HEAD: C2 = AWR
            continue;
        }
        if head.mf != 3 || head.mt == 0 {
            continue;
        }
        // Section structure: HEAD (this line), TAB1 header, INTTAB
        // line(s), TAB1 data line(s), SEND.
        let Some(tab1) = lines.next() else { break };
        let tab1 = parse_record(tab1);
        let (nr, np) = (tab1.n1.max(0) as usize, tab1.n2.max(0) as usize);
        // INTTAB: (NBT,INT) pairs packed three per line across the six
        // data fields (C1,C2 / L1,L2 / N1,N2), as integers.
        let mut regions = Vec::with_capacity(nr);
        let int_lines = nr.div_ceil(3);
        for _ in 0..int_lines {
            let Some(il) = lines.next() else { break };
            let r = parse_record(il);
            let pairs: [(i64, i64); 3] = [(r.c1 as i64, r.c2 as i64), (r.l1, r.l2), (r.n1, r.n2)];
            for (nbt, int) in pairs {
                if regions.len() < nr {
                    regions.push((nbt as u32, int as u32));
                }
            }
        }
        // TAB1 data: (E,σ) pairs, three per line across the six fields —
        // all read as reals here.
        let mut energy_ev = Vec::with_capacity(np);
        let mut xs_barns = Vec::with_capacity(np);
        let data_lines = np.div_ceil(3);
        for _ in 0..data_lines {
            let Some(dl) = lines.next() else { break };
            let f = |a: usize, b: usize| endf_float(dl.get(a..b).unwrap_or(""));
            for (e, s) in [
                (f(0, 11), f(11, 22)),
                (f(22, 33), f(33, 44)),
                (f(44, 55), f(55, 66)),
            ] {
                if energy_ev.len() < np {
                    energy_ev.push(e);
                    xs_barns.push(s);
                }
            }
        }
        if energy_ev.len() == np && !regions.is_empty() {
            sections.insert(
                head.mt as u32,
                EndfXsSection {
                    q_ev: tab1.c2.abs(),
                    regions,
                    energy_ev,
                    xs_barns,
                },
            );
        }
    }
    if sections.is_empty() {
        return Err("no MF3 sections parsed".into());
    }
    Ok(EndfTape { awr, sections })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fortran_float_variants() {
        assert!((endf_float(" 1.234567+08") - 1.234567e8).abs() < 1e-6);
        assert!((endf_float(" 1.234567E-38") - 1.234567e-38).abs() < 1e-44);
        assert!((endf_float("-2.500000+3") + 2500.0).abs() < 1e-9);
        assert_eq!(endf_float(""), 0.0);
        assert_eq!(endf_float("  0.000000+00"), 0.0);
    }

    #[test]
    fn parses_minimal_mf3_section() {
        // One MF3/MT2 section: HEAD, TAB1 (1 region, 4 points, law 2),
        // INTTAB, data, SEND.
        let tape = format!(
            "{}\n{}\n{}\n{}\n{}\n{}\n",
            " 1.001000+3 1.008665+0          0          0          0          01001 1451    1",
            " 1.001000+3 1.008665+0          0          0          0          01001 3  2    2",
            " 0.000000+0 1.000000+0          0          0          1          41001 3  2    3",
            "          4          2                                  1001 3  2    4",
            " 1.000000-5 2.000000+1 2.530000-2 2.050000+1 1.000000+4 1.000000+11001 3  2    5",
            " 1.000000+5 1.000000+0                                            1001 3  2    6",
        );
        let tape = parse_endf(&tape).unwrap();
        assert!((tape.awr - 1.008665).abs() < 1e-6);
        let s = tape.sections.get(&2).unwrap();
        assert_eq!(s.energy_ev.len(), 4);
        // Linear interpolation: midpoint between σ=20 and 20.5.
        let v = s.at(0.01265);
        assert!((v - 20.25).abs() < 0.05, "got {v}");
    }
}
