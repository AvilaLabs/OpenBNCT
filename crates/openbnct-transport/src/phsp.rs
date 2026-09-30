// SPDX-License-Identifier: MIT

//! Streaming reader (and a small writer, for fixtures and converters) for
//! IAEA phase-space files: an ASCII `.IAEAheader` plus a binary
//! `.IAEAphsp` record file (Capote et al., IAEA INDC(NDS)-0484, 2006).
//!
//! Record layout, in file order: particle type (`i8`, negative on the
//! first particle of a new history), kinetic energy in MeV (`f32`, the
//! sign carries the sign of the direction cosine W), then — each only if
//! the header stores it — X, Y, Z in cm, U, V, [W], statistical weight
//! (`f32`), the extra floats (`f32`) and the extra longs (`i32`). A
//! variable the header does not store takes its `RECORD_CONSTANT` value.
//! W is reconstructed as `sign · sqrt(1 − U² − V²)` when it is not
//! physically present in the record.
//!
//! The reader streams one record at a time through a fixed buffer, so a
//! multi-gigabyte file costs constant memory.

use std::fs::File;
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};

use thiserror::Error;

/// IAEA particle type codes.
pub const PHSP_PHOTON: i8 = 1;
pub const PHSP_ELECTRON: i8 = 2;
pub const PHSP_POSITRON: i8 = 3;
pub const PHSP_NEUTRON: i8 = 4;
pub const PHSP_PROTON: i8 = 5;

#[derive(Debug, Error)]
pub enum PhspError {
    #[error("phase-space I/O on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid IAEA header: {0}")]
    Header(String),
    #[error("invalid IAEA phase-space data: {0}")]
    Data(String),
}

/// Parsed `.IAEAheader` — the keys the reader needs plus the declared
/// particle counts.
#[derive(Debug, Clone, PartialEq)]
pub struct IaeaHeader {
    /// `X, Y, Z, U, V, W, Weight` stored flags.
    pub stored: [bool; 7],
    pub extra_floats: usize,
    pub extra_longs: usize,
    /// Values used for variables that are not stored: X, Y, Z, U, V, W,
    /// weight.
    pub constants: [f64; 7],
    pub record_length: usize,
    pub big_endian: bool,
    /// Whether W occupies bytes in the record (see [`IaeaHeader::parse`]).
    pub w_in_record: bool,
    pub original_histories: Option<f64>,
    pub particles: Option<u64>,
    pub photons: Option<u64>,
    pub electrons: Option<u64>,
    pub positrons: Option<u64>,
    pub neutrons: Option<u64>,
    pub protons: Option<u64>,
    pub title: Option<String>,
}

fn header_sections(text: &str) -> Vec<(String, Vec<String>)> {
    let mut sections: Vec<(String, Vec<String>)> = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if let Some(rest) = line.strip_prefix('$') {
            let key = rest
                .trim()
                .trim_end_matches(':')
                .trim()
                .to_ascii_uppercase();
            sections.push((key, Vec::new()));
        } else if let Some(section) = sections.last_mut() {
            section.1.push(raw.to_string());
        }
    }
    sections
}

fn numeric_tokens(lines: &[String]) -> Vec<f64> {
    let mut out = Vec::new();
    for line in lines {
        let content = line.split("//").next().unwrap_or("");
        for token in content.split_whitespace() {
            if let Ok(value) = token.parse::<f64>() {
                out.push(value);
            }
        }
    }
    out
}

impl IaeaHeader {
    /// Parse header text. Sections start at a `$KEY:` line; `//` starts a
    /// comment. `RECORD_CONTENTS` carries nine integers (X, Y, Z, U, V, W,
    /// weight stored flags, then the extra-float and extra-long counts).
    ///
    /// Some writers set the W flag although W is derived from U and V and
    /// takes no bytes; the flag is honoured only when it makes
    /// `RECORD_LENGTH` consistent, otherwise W is taken as derived. If
    /// neither reading matches `RECORD_LENGTH` the header is rejected.
    pub fn parse(text: &str) -> Result<Self, PhspError> {
        let err = |m: &str| PhspError::Header(m.to_string());
        let sections = header_sections(text);
        let find = |key: &str| sections.iter().find(|(k, _)| k == key).map(|(_, v)| v);

        let contents =
            numeric_tokens(find("RECORD_CONTENTS").ok_or_else(|| err("missing $RECORD_CONTENTS"))?);
        if contents.len() < 9 {
            return Err(err("$RECORD_CONTENTS needs 9 integers"));
        }
        let mut stored = [false; 7];
        for (slot, value) in stored.iter_mut().zip(&contents[..7]) {
            *slot = *value != 0.0;
        }
        let count = |value: f64, what: &str| -> Result<usize, PhspError> {
            if value >= 0.0 && value.fract() == 0.0 && value < 4096.0 {
                Ok(value as usize)
            } else {
                Err(PhspError::Header(format!(
                    "{what} count {value} is invalid"
                )))
            }
        };
        let extra_floats = count(contents[7], "extra floats")?;
        let extra_longs = count(contents[8], "extra longs")?;

        let mut constants = [0.0; 7];
        if let Some(lines) = find("RECORD_CONSTANT") {
            for (slot, value) in constants.iter_mut().zip(numeric_tokens(lines)) {
                *slot = value;
            }
        }

        let record_length =
            numeric_tokens(find("RECORD_LENGTH").ok_or_else(|| err("missing $RECORD_LENGTH"))?)
                .first()
                .copied()
                .filter(|v| *v > 0.0 && v.fract() == 0.0)
                .ok_or_else(|| err("$RECORD_LENGTH must be a positive integer"))?
                as usize;

        let order = numeric_tokens(find("BYTE_ORDER").ok_or_else(|| err("missing $BYTE_ORDER"))?)
            .first()
            .copied()
            .unwrap_or(0.0);
        let big_endian = match order as i64 {
            1234 => false,
            4321 => true,
            other => {
                return Err(PhspError::Header(format!(
                    "$BYTE_ORDER must be 1234 (little) or 4321 (big), got {other}"
                )));
            }
        };

        let fixed = |w_in_record: bool| -> usize {
            let mut n = 1 + 4;
            for (i, s) in stored.iter().enumerate() {
                if *s && (i != 5 || w_in_record) {
                    n += 4;
                }
            }
            n + 4 * extra_floats + 4 * extra_longs
        };
        let w_in_record = if stored[5] && fixed(true) == record_length {
            true
        } else if fixed(false) == record_length {
            false
        } else {
            return Err(PhspError::Header(format!(
                "$RECORD_LENGTH {record_length} does not match the stored variables \
                 (expected {} or {} bytes)",
                fixed(false),
                fixed(true)
            )));
        };

        let opt_count = |key: &str| -> Option<u64> {
            find(key)
                .and_then(|lines| numeric_tokens(lines).first().copied())
                .filter(|v| *v >= 0.0)
                .map(|v| v as u64)
        };
        let title = find("TITLE").and_then(|lines| {
            lines
                .iter()
                .map(|l| l.trim())
                .find(|l| !l.is_empty())
                .map(str::to_string)
        });
        Ok(Self {
            stored,
            extra_floats,
            extra_longs,
            constants,
            record_length,
            big_endian,
            w_in_record,
            original_histories: find("ORIGINAL_HISTORIES")
                .and_then(|lines| numeric_tokens(lines).first().copied()),
            particles: opt_count("PARTICLES"),
            photons: opt_count("PHOTONS"),
            electrons: opt_count("ELECTRONS"),
            positrons: opt_count("POSITRONS"),
            neutrons: opt_count("NEUTRONS"),
            protons: opt_count("PROTONS"),
            title,
        })
    }
}

/// One phase-space particle, in the file's own frame (cm, MeV).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PhspRecord {
    /// IAEA particle type (1 photon, 2 electron, 3 positron, 4 neutron,
    /// 5 proton), always positive.
    pub particle_type: i8,
    /// True on the first particle of a new original history.
    pub new_history: bool,
    pub energy_mev: f64,
    pub position_cm: [f64; 3],
    /// Unit direction (U, V, W).
    pub direction: [f64; 3],
    pub weight: f64,
}

/// Sibling data path for a header path: `x.IAEAheader` → `x.IAEAphsp`.
#[must_use]
pub fn phsp_data_path(header_path: &Path) -> PathBuf {
    header_path.with_extension("IAEAphsp")
}

/// Streaming record reader.
pub struct PhspReader {
    pub header: IaeaHeader,
    pub data_path: PathBuf,
    reader: BufReader<File>,
    buffer: Vec<u8>,
    remaining: u64,
    /// Total records in the data file.
    pub total_records: u64,
}

impl PhspReader {
    /// Open a header and its sibling `.IAEAphsp`. The data file size must
    /// be a whole number of records and, when the header declares
    /// `$PARTICLES`, agree with it.
    pub fn open(header_path: &Path) -> Result<Self, PhspError> {
        let io = |path: &Path, source| PhspError::Io {
            path: path.to_path_buf(),
            source,
        };
        let text = std::fs::read_to_string(header_path).map_err(|e| io(header_path, e))?;
        let header = IaeaHeader::parse(&text)?;
        let data_path = phsp_data_path(header_path);
        let file = File::open(&data_path).map_err(|e| io(&data_path, e))?;
        let size = file.metadata().map_err(|e| io(&data_path, e))?.len();
        let length = header.record_length as u64;
        if size % length != 0 {
            return Err(PhspError::Data(format!(
                "{} is {size} bytes, not a multiple of the {length}-byte record",
                data_path.display()
            )));
        }
        let total_records = size / length;
        if let Some(declared) = header.particles
            && declared != total_records
        {
            return Err(PhspError::Data(format!(
                "header declares {declared} particles but {} holds {total_records} records",
                data_path.display()
            )));
        }
        Ok(Self {
            buffer: vec![0; header.record_length],
            header,
            data_path,
            reader: BufReader::with_capacity(1 << 20, file),
            remaining: total_records,
            total_records,
        })
    }

    /// Read the next record; `Ok(None)` at end of file.
    pub fn next_record(&mut self) -> Result<Option<PhspRecord>, PhspError> {
        if self.remaining == 0 {
            return Ok(None);
        }
        self.reader
            .read_exact(&mut self.buffer)
            .map_err(|source| PhspError::Io {
                path: self.data_path.clone(),
                source,
            })?;
        self.remaining -= 1;
        let h = &self.header;
        let b = &self.buffer;
        let f32_at = |offset: usize| -> f32 {
            let bytes = [b[offset], b[offset + 1], b[offset + 2], b[offset + 3]];
            if h.big_endian {
                f32::from_be_bytes(bytes)
            } else {
                f32::from_le_bytes(bytes)
            }
        };
        let type_byte = b[0] as i8;
        let new_history = type_byte < 0;
        let particle_type = type_byte.saturating_abs();
        let energy_signed = f64::from(f32_at(1));
        let mut offset = 5;
        let mut values = h.constants;
        for (i, slot) in values.iter_mut().enumerate() {
            if h.stored[i] && (i != 5 || h.w_in_record) {
                *slot = f64::from(f32_at(offset));
                offset += 4;
            }
        }
        let w_negative = energy_signed.is_sign_negative();
        let (u, v) = (values[3], values[4]);
        let w = if h.w_in_record || (!h.stored[3] && !h.stored[4] && !h.stored[5]) {
            // Stored (or constant) W: the sign travels with the energy
            // sign when W is stored; for a fully constant direction the
            // declared constant already has its sign.
            if h.w_in_record {
                let magnitude = values[5].abs();
                if w_negative { -magnitude } else { magnitude }
            } else {
                values[5]
            }
        } else {
            let magnitude = (1.0 - u * u - v * v).max(0.0).sqrt();
            if w_negative { -magnitude } else { magnitude }
        };
        Ok(Some(PhspRecord {
            particle_type,
            new_history,
            energy_mev: energy_signed.abs(),
            position_cm: [values[0], values[1], values[2]],
            direction: [u, v, w],
            weight: values[6],
        }))
    }
}

impl Iterator for PhspReader {
    type Item = Result<PhspRecord, PhspError>;
    fn next(&mut self) -> Option<Self::Item> {
        self.next_record().transpose()
    }
}

/// Summary statistics of one particle type, gathered by a full scan.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PhspTypeSummary {
    pub records: u64,
    pub weight_sum: f64,
    pub min_energy_mev: f64,
    pub max_energy_mev: f64,
    pub mean_energy_mev: f64,
}

/// Result of scanning a whole phase-space file.
#[derive(Debug, Clone, PartialEq)]
pub struct PhspScan {
    pub records: u64,
    /// Records with the new-history flag set.
    pub histories: u64,
    /// Summaries keyed by IAEA particle type.
    pub by_type: std::collections::BTreeMap<i8, PhspTypeSummary>,
    pub min_position_cm: [f64; 3],
    pub max_position_cm: [f64; 3],
}

/// Full streaming scan: counts and weight sums per particle type.
pub fn scan_phsp(reader: &mut PhspReader) -> Result<PhspScan, PhspError> {
    let mut scan = PhspScan {
        records: 0,
        histories: 0,
        by_type: Default::default(),
        min_position_cm: [f64::INFINITY; 3],
        max_position_cm: [f64::NEG_INFINITY; 3],
    };
    let mut energy_weighted: std::collections::BTreeMap<i8, f64> = Default::default();
    while let Some(r) = reader.next_record()? {
        scan.records += 1;
        scan.histories += u64::from(r.new_history);
        let entry = scan
            .by_type
            .entry(r.particle_type)
            .or_insert(PhspTypeSummary {
                min_energy_mev: f64::INFINITY,
                max_energy_mev: f64::NEG_INFINITY,
                ..Default::default()
            });
        entry.records += 1;
        entry.weight_sum += r.weight;
        entry.min_energy_mev = entry.min_energy_mev.min(r.energy_mev);
        entry.max_energy_mev = entry.max_energy_mev.max(r.energy_mev);
        *energy_weighted.entry(r.particle_type).or_insert(0.0) += r.weight * r.energy_mev;
        for k in 0..3 {
            scan.min_position_cm[k] = scan.min_position_cm[k].min(r.position_cm[k]);
            scan.max_position_cm[k] = scan.max_position_cm[k].max(r.position_cm[k]);
        }
    }
    for (kind, summary) in &mut scan.by_type {
        if summary.weight_sum != 0.0 {
            summary.mean_energy_mev = energy_weighted[kind] / summary.weight_sum;
        }
    }
    Ok(scan)
}

/// Write an IAEA phase-space pair for fixtures and converters: X, Y, Z,
/// U, V and weight stored (W derived), little-endian, no extras. Returns
/// the header path. `original_histories` is recorded verbatim.
pub fn write_iaea_phsp(
    header_path: &Path,
    records: &[PhspRecord],
    original_histories: f64,
    title: &str,
) -> Result<(), PhspError> {
    let io = |path: &Path, source| PhspError::Io {
        path: path.to_path_buf(),
        source,
    };
    let data_path = phsp_data_path(header_path);
    let mut data =
        std::io::BufWriter::new(File::create(&data_path).map_err(|e| io(&data_path, e))?);
    let mut counts = [0_u64; 6];
    for r in records {
        let code = if r.new_history {
            -r.particle_type
        } else {
            r.particle_type
        };
        let mut energy = r.energy_mev as f32;
        if r.direction[2] < 0.0 {
            energy = -energy;
        }
        let mut bytes = Vec::with_capacity(29);
        bytes.push(code as u8);
        bytes.extend_from_slice(&energy.to_le_bytes());
        for value in [
            r.position_cm[0],
            r.position_cm[1],
            r.position_cm[2],
            r.direction[0],
            r.direction[1],
            r.weight,
        ] {
            bytes.extend_from_slice(&(value as f32).to_le_bytes());
        }
        data.write_all(&bytes).map_err(|e| io(&data_path, e))?;
        if (1..=5).contains(&r.particle_type) {
            counts[r.particle_type as usize] += 1;
        }
    }
    data.flush().map_err(|e| io(&data_path, e))?;
    let header = format!(
        "$IAEA_INDEX:\n0\n\n$TITLE:\n{title}\n\n$FILE_TYPE:\n0\n\n$RECORD_CONTENTS:\n\
         1 // X is stored ?\n1 // Y is stored ?\n1 // Z is stored ?\n1 // U is stored ?\n\
         1 // V is stored ?\n1 // W is stored ?\n1 // Weight is stored ?\n0 // Extra floats stored ?\n\
         0 // Extra longs stored ?\n\n$RECORD_CONSTANT:\n0 // X\n0 // Y\n0 // Z\n0 // U\n0 // V\n\
         0 // W\n0 // Weight\n\n$RECORD_LENGTH:\n29\n\n$BYTE_ORDER:\n1234\n\n\
         $ORIGINAL_HISTORIES:\n{original_histories}\n\n$PARTICLES:\n{}\n\n$PHOTONS:\n{}\n\n\
         $ELECTRONS:\n{}\n\n$POSITRONS:\n{}\n\n$NEUTRONS:\n{}\n\n$PROTONS:\n{}\n",
        records.len(),
        counts[1],
        counts[2],
        counts[3],
        counts[4],
        counts[5]
    );
    std::fs::write(header_path, header).map_err(|e| io(header_path, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(kind: i8, new: bool, e: f64, pos: [f64; 3], dir: [f64; 3], w: f64) -> PhspRecord {
        PhspRecord {
            particle_type: kind,
            new_history: new,
            energy_mev: e,
            position_cm: pos,
            direction: dir,
            weight: w,
        }
    }

    #[test]
    fn round_trip_neutrons_photons_and_backward_sign() {
        let dir = tempdir();
        let header = dir.join("t.IAEAheader");
        let d = 0.6_f64;
        let wz = (1.0_f64 - d * d - 0.0).sqrt();
        let records = vec![
            record(PHSP_NEUTRON, true, 1.5, [1.0, -2.0, 0.0], [d, 0.0, wz], 0.5),
            record(PHSP_PHOTON, false, 0.25, [0.0, 0.0, 0.0], [0.0, d, wz], 2.0),
            record(
                PHSP_NEUTRON,
                true,
                3.0,
                [-1.5, 0.5, 0.0],
                [0.0, d, -wz],
                1.0,
            ),
        ];
        write_iaea_phsp(&header, &records, 2.0, "unit test").unwrap();
        let mut reader = PhspReader::open(&header).unwrap();
        assert_eq!(reader.total_records, 3);
        assert_eq!(reader.header.neutrons, Some(2));
        assert_eq!(reader.header.photons, Some(1));
        assert_eq!(reader.header.original_histories, Some(2.0));
        assert!(
            !reader.header.w_in_record,
            "W flag set but record length excludes it"
        );
        let back: Vec<_> = (&mut reader).collect::<Result<_, _>>().unwrap();
        assert_eq!(back.len(), 3);
        for (a, b) in records.iter().zip(&back) {
            assert_eq!(a.particle_type, b.particle_type);
            assert_eq!(a.new_history, b.new_history);
            assert!((a.energy_mev - b.energy_mev).abs() < 1e-6);
            for k in 0..3 {
                assert!((a.position_cm[k] - b.position_cm[k]).abs() < 1e-6);
                assert!((a.direction[k] - b.direction[k]).abs() < 1e-6, "{k}");
            }
            assert!((a.weight - b.weight).abs() < 1e-6);
        }
        assert!(back[2].direction[2] < 0.0);
        let mut reader = PhspReader::open(&header).unwrap();
        let scan = scan_phsp(&mut reader).unwrap();
        assert_eq!(scan.histories, 2);
        assert_eq!(scan.by_type[&PHSP_NEUTRON].records, 2);
        assert!((scan.by_type[&PHSP_NEUTRON].weight_sum - 1.5).abs() < 1e-9);
        assert_eq!(scan.by_type[&PHSP_PHOTON].records, 1);
    }

    #[test]
    fn constant_z_big_endian_and_extras() {
        let dir = tempdir();
        let header = dir.join("c.IAEAheader");
        // X,Y stored, Z/U/V/W/weight constant except U,V stored; one extra long.
        let text = "$RECORD_CONTENTS:\n1\n1\n0\n1\n1\n0\n0\n0\n1\n$RECORD_CONSTANT:\n0\n0\n7.5\n0\n0\n1\n0.25\n\
                    $RECORD_LENGTH:\n25\n$BYTE_ORDER:\n4321\n$PARTICLES:\n1\n";
        std::fs::write(&header, text).unwrap();
        let mut rec = vec![4_u8];
        rec.extend_from_slice(&2.0_f32.to_be_bytes());
        for v in [1.0_f32, 2.0, 0.0, 0.8] {
            rec.extend_from_slice(&v.to_be_bytes());
        }
        rec.extend_from_slice(&42_i32.to_be_bytes());
        assert_eq!(rec.len(), 25);
        std::fs::write(phsp_data_path(&header), rec).unwrap();
        let mut reader = PhspReader::open(&header).unwrap();
        let r = reader.next_record().unwrap().unwrap();
        assert_eq!(r.particle_type, 4);
        assert!(!r.new_history);
        assert_eq!(r.position_cm, [1.0, 2.0, 7.5]);
        assert!((r.direction[0] - 0.0).abs() < 1e-7 && (r.direction[1] - 0.8).abs() < 1e-7);
        assert!((r.direction[2] - 0.6).abs() < 1e-6);
        assert_eq!(r.weight, 0.25);
        assert!(reader.next_record().unwrap().is_none());
    }

    #[test]
    fn rejects_inconsistent_length_and_truncation() {
        let text = "$RECORD_CONTENTS:\n1\n1\n1\n1\n1\n0\n1\n0\n0\n$RECORD_LENGTH:\n99\n$BYTE_ORDER:\n1234\n";
        assert!(matches!(IaeaHeader::parse(text), Err(PhspError::Header(_))));
        let dir = tempdir();
        let header = dir.join("bad.IAEAheader");
        let ok = "$RECORD_CONTENTS:\n1\n1\n1\n1\n1\n0\n1\n0\n0\n$RECORD_LENGTH:\n29\n$BYTE_ORDER:\n1234\n";
        std::fs::write(&header, ok).unwrap();
        std::fs::write(phsp_data_path(&header), [0_u8; 30]).unwrap();
        assert!(matches!(PhspReader::open(&header), Err(PhspError::Data(_))));
        let bad_order = ok.replace("1234", "9999");
        assert!(matches!(
            IaeaHeader::parse(&bad_order),
            Err(PhspError::Header(_))
        ));
    }

    fn tempdir() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "openbnct-phsp-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
