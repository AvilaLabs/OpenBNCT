// SPDX-License-Identifier: MIT

//! Exact dose statistics over masked voxel selections.
//!
//! These are pure functions shared by the evidence layer (dose-volume
//! metrics) and the biological layer (scalar dose statistics consumed by
//! endpoint models). They implement no scientific interpretation beyond the
//! stated definitions; every caller is responsible for recording which dose
//! quantity and unit the statistic was taken over.

use crate::ValidationError;

/// Collect the values inside `mask`, requiring matching lengths, at least
/// one selected voxel, and finite non-negative doses.
pub fn masked_values(
    name: &str,
    values: &[f64],
    mask: &[bool],
) -> Result<Vec<f64>, ValidationError> {
    if values.len() != mask.len() {
        return Err(ValidationError::MaskValuesLength {
            mask: name.into(),
            mask_voxels: mask.len(),
            values: values.len(),
        });
    }
    let mut selected = Vec::new();
    for (index, inside) in mask.iter().enumerate() {
        if !inside {
            continue;
        }
        let value = values[index];
        if !value.is_finite() || value < 0.0 {
            return Err(ValidationError::InvalidMaskedDose {
                mask: name.into(),
                index,
            });
        }
        selected.push(value);
    }
    if selected.is_empty() {
        return Err(ValidationError::EmptyMask(name.into()));
    }
    Ok(selected)
}

/// Arithmetic mean of a non-empty selection.
pub fn mean(selected: &[f64]) -> f64 {
    selected.iter().sum::<f64>() / selected.len() as f64
}

/// `D_x`: the dose level such that x percent of the volume receives at
/// least that dose. `percent` must lie in `(0, 100]`; `D_100` is the
/// minimum dose. The selection is sorted ascending and the value at the
/// continuous position `(1 - x/100) * (n - 1)` is linearly interpolated —
/// the standard discrete-DVH reading for equal-volume voxels.
pub fn dose_covering_percent(selected: &[f64], percent: f64) -> Result<f64, ValidationError> {
    if selected.is_empty() {
        return Err(ValidationError::EmptyMask("dx".into()));
    }
    if !percent.is_finite() || percent <= 0.0 || percent > 100.0 {
        return Err(ValidationError::InvalidStatistic {
            name: "dx_percent",
            reason: format!("coverage percent {percent} must lie in (0, 100]"),
        });
    }
    let mut sorted = selected.to_vec();
    sorted.sort_by(f64::total_cmp);
    let position = (1.0 - percent / 100.0) * (sorted.len() - 1) as f64;
    let lower = position.floor() as usize;
    let upper = (lower + 1).min(sorted.len() - 1);
    let fraction = position - lower as f64;
    Ok(sorted[lower] * (1.0 - fraction) + sorted[upper] * fraction)
}

/// `V_x`: the fraction of the selection receiving at least `level` dose.
pub fn volume_at_least(selected: &[f64], level: f64) -> Result<f64, ValidationError> {
    if selected.is_empty() {
        return Err(ValidationError::EmptyMask("vx".into()));
    }
    if !level.is_finite() || level < 0.0 {
        return Err(ValidationError::InvalidStatistic {
            name: "vx_level",
            reason: format!("dose level {level} must be finite non-negative"),
        });
    }
    let covered = selected.iter().filter(|value| **value >= level).count();
    Ok(covered as f64 / selected.len() as f64)
}

/// Generalized equivalent uniform dose `(mean_i D_i^a)^(1/a)`.
///
/// `a` is the Niemierko organ parameter: `a = 1` reduces to the mean,
/// `a → +∞` approaches the maximum (serial behavior), `a → −∞` the
/// minimum (parallel behavior). The `a = 0` limit is the geometric mean.
/// Any zero-dose voxel yields `EUD = 0` for `a <= 0`: an unirradiated
/// voxel collapses a parallel organ's equivalent dose.
pub fn equivalent_uniform_dose(selected: &[f64], a: f64) -> Result<f64, ValidationError> {
    if selected.is_empty() {
        return Err(ValidationError::EmptyMask("eud".into()));
    }
    if !a.is_finite() {
        return Err(ValidationError::InvalidStatistic {
            name: "eud_parameter",
            reason: "organ parameter must be finite".into(),
        });
    }
    if a == 0.0 {
        if selected.contains(&0.0) {
            return Ok(0.0);
        }
        let log_mean = selected.iter().map(|value| value.ln()).sum::<f64>() / selected.len() as f64;
        return Ok(log_mean.exp());
    }
    if a < 0.0 && selected.contains(&0.0) {
        return Ok(0.0);
    }
    // Normalize before the power mean: `scale * (mean (D/scale)^a)^(1/a)`
    // keeps the powered terms in (0, 1] so large |a| cannot overflow.
    let scale = if a > 0.0 {
        selected.iter().copied().fold(0.0, f64::max)
    } else {
        // a < 0 with no zero doses: the minimum is strictly positive.
        selected.iter().copied().fold(f64::INFINITY, f64::min)
    };
    if scale == 0.0 {
        return Ok(0.0);
    }
    let powered = selected
        .iter()
        .map(|value| (value / scale).powf(a))
        .sum::<f64>()
        / selected.len() as f64;
    if !powered.is_finite() || powered < 0.0 {
        return Err(ValidationError::InvalidStatistic {
            name: "eud_parameter",
            reason: "power mean overflowed or is undefined for these doses".into(),
        });
    }
    Ok(scale * powered.powf(1.0 / a))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masked_values_selects_and_validates() {
        assert_eq!(
            masked_values("m", &[1.0, -1.0, 3.0], &[true, false, true]).unwrap(),
            vec![1.0, 3.0]
        );
        assert!(masked_values("m", &[1.0], &[true, true]).is_err());
        assert!(masked_values("m", &[1.0], &[false]).is_err());
        assert!(masked_values("m", &[f64::NAN], &[true]).is_err());
    }

    #[test]
    fn dx_reads_the_discrete_dvh() {
        let doses = vec![0.0, 1.0, 2.0, 3.0, 4.0];
        // D100 = min = 0, D50 interpolates at position 0.5*4 = 2 -> 2.0.
        assert_eq!(dose_covering_percent(&doses, 100.0).unwrap(), 0.0);
        assert_eq!(dose_covering_percent(&doses, 50.0).unwrap(), 2.0);
        // D25 -> position 0.75*4 = 3 -> 3.0; D90 -> position 0.4 ->
        // interpolate s[0] and s[1] at 0.4 -> 0.4.
        assert_eq!(dose_covering_percent(&doses, 25.0).unwrap(), 3.0);
        assert!((dose_covering_percent(&doses, 90.0).unwrap() - 0.4).abs() < 1e-12);
        assert!(dose_covering_percent(&doses, 0.0).is_err());
        assert!(dose_covering_percent(&doses, 101.0).is_err());
    }

    #[test]
    fn vx_counts_covered_fraction() {
        let doses = vec![0.0, 1.0, 2.0, 3.0];
        assert_eq!(volume_at_least(&doses, 0.0).unwrap(), 1.0);
        assert_eq!(volume_at_least(&doses, 1.0).unwrap(), 0.75);
        assert_eq!(volume_at_least(&doses, 4.0).unwrap(), 0.0);
        assert!(volume_at_least(&doses, -1.0).is_err());
    }

    #[test]
    fn eud_covers_the_standard_limits() {
        let doses = vec![1.0, 2.0, 3.0, 4.0];
        assert_eq!(equivalent_uniform_dose(&doses, 1.0).unwrap(), 2.5);
        // a -> +inf approaches max; a=1000 lands within fp tolerance.
        let serial = equivalent_uniform_dose(&doses, 1000.0).unwrap();
        assert!((serial - 4.0).abs() < 0.01);
        // a -> -inf approaches min.
        let parallel = equivalent_uniform_dose(&doses, -1000.0).unwrap();
        assert!((parallel - 1.0).abs() < 0.01);
        // a = 0 is the geometric mean.
        let geo = equivalent_uniform_dose(&doses, 0.0).unwrap();
        assert!((geo - (24.0_f64).powf(0.25)).abs() < 1e-12);
        // Zero dose collapses parallel and geometric EUD but not serial.
        let with_zero = vec![0.0, 2.0];
        assert_eq!(equivalent_uniform_dose(&with_zero, -5.0).unwrap(), 0.0);
        assert_eq!(equivalent_uniform_dose(&with_zero, 0.0).unwrap(), 0.0);
        assert!(equivalent_uniform_dose(&with_zero, 2.0).unwrap() > 0.0);
        assert!(equivalent_uniform_dose(&doses, f64::INFINITY).is_err());
    }
}
