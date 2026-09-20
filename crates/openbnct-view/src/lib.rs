// SPDX-License-Identifier: Apache-2.0

//! UI-independent geometry for linked anatomical slice views.
//!
//! R1 intentionally exposes anatomical labels only for grids aligned to the
//! canonical DICOM LPS patient axes. Oblique or permuted grids are rejected so
//! a GUI cannot display confident but incorrect `R/L`, `A/P`, or `S/I` labels.

#![forbid(unsafe_code)]

use openbnct_core::GridGeometry;
use thiserror::Error;

const ALIGNMENT_TOLERANCE: f64 = 1.0e-6;
const IDENTITY_DIRECTION: [f64; 9] = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AnatomicalPlane {
    Axial,
    Coronal,
    Sagittal,
}

impl AnatomicalPlane {
    pub const ALL: [Self; 3] = [Self::Axial, Self::Coronal, Self::Sagittal];

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Axial => "Axial",
            Self::Coronal => "Coronal",
            Self::Sagittal => "Sagittal",
        }
    }

    #[must_use]
    pub const fn edge_labels(self) -> EdgeLabels {
        match self {
            Self::Axial => EdgeLabels {
                left: "R",
                right: "L",
                top: "A",
                bottom: "P",
            },
            Self::Coronal => EdgeLabels {
                left: "R",
                right: "L",
                top: "S",
                bottom: "I",
            },
            Self::Sagittal => EdgeLabels {
                left: "A",
                right: "P",
                top: "S",
                bottom: "I",
            },
        }
    }

    /// The volume axis this plane slices along — axial fixes z,
    /// coronal fixes y, sagittal fixes x.
    #[must_use]
    pub const fn fixed_axis(self) -> usize {
        match self {
            Self::Axial => 2,
            Self::Coronal => 1,
            Self::Sagittal => 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EdgeLabels {
    pub left: &'static str,
    pub right: &'static str,
    pub top: &'static str,
    pub bottom: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Crosshair {
    voxel: [u32; 3],
}

impl Crosshair {
    pub fn centered(grid: &PatientAlignedGrid) -> Self {
        Self {
            voxel: grid.geometry.shape.map(|extent| extent / 2),
        }
    }

    pub fn new(grid: &PatientAlignedGrid, voxel: [u32; 3]) -> Result<Self, ViewError> {
        validate_voxel(&grid.geometry, voxel)?;
        Ok(Self { voxel })
    }

    #[must_use]
    pub const fn voxel(self) -> [u32; 3] {
        self.voxel
    }

    pub fn set_voxel(
        &mut self,
        grid: &PatientAlignedGrid,
        voxel: [u32; 3],
    ) -> Result<(), ViewError> {
        validate_voxel(&grid.geometry, voxel)?;
        self.voxel = voxel;
        Ok(())
    }

    pub fn select_pixel(
        &mut self,
        grid: &PatientAlignedGrid,
        view: &SliceView,
        pixel: [u32; 2],
    ) -> Result<(), ViewError> {
        self.set_voxel(grid, view.voxel_at(pixel)?)
    }

    pub fn world_lps_mm(self, grid: &PatientAlignedGrid) -> Result<[f64; 3], ViewError> {
        grid.geometry
            .voxel_center_lps_mm(self.voxel)
            .map_err(|error| ViewError::InvalidGeometry(error.to_string()))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PatientAlignedGrid {
    geometry: GridGeometry,
    voxel_count: usize,
}

impl PatientAlignedGrid {
    pub fn new(geometry: &GridGeometry) -> Result<Self, ViewError> {
        let voxel_count = geometry
            .voxel_count()
            .map_err(|error| ViewError::InvalidGeometry(error.to_string()))?;
        if geometry
            .direction
            .into_iter()
            .zip(IDENTITY_DIRECTION)
            .any(|(observed, expected)| (observed - expected).abs() > ALIGNMENT_TOLERANCE)
        {
            return Err(ViewError::NotPatientAligned(geometry.direction));
        }
        Ok(Self {
            geometry: geometry.clone(),
            voxel_count,
        })
    }

    #[must_use]
    pub fn geometry(&self) -> &GridGeometry {
        &self.geometry
    }

    #[must_use]
    pub const fn voxel_count(&self) -> usize {
        self.voxel_count
    }

    pub fn linear_index(&self, voxel: [u32; 3]) -> Result<usize, ViewError> {
        validate_voxel(&self.geometry, voxel)?;
        Ok(linear_index(self.geometry.shape, voxel))
    }

    pub fn slice(
        &self,
        plane: AnatomicalPlane,
        crosshair: Crosshair,
    ) -> Result<SliceView, ViewError> {
        validate_voxel(&self.geometry, crosshair.voxel)?;
        let dimensions = match plane {
            AnatomicalPlane::Axial => [self.geometry.shape[0], self.geometry.shape[1]],
            AnatomicalPlane::Coronal => [self.geometry.shape[0], self.geometry.shape[2]],
            AnatomicalPlane::Sagittal => [self.geometry.shape[1], self.geometry.shape[2]],
        };
        Ok(SliceView {
            plane,
            fixed_index: crosshair.voxel[plane.fixed_axis()],
            dimensions,
            volume_shape: self.geometry.shape,
            pixel_spacing_mm: match plane {
                AnatomicalPlane::Axial => {
                    [self.geometry.spacing_mm[0], self.geometry.spacing_mm[1]]
                }
                AnatomicalPlane::Coronal => {
                    [self.geometry.spacing_mm[0], self.geometry.spacing_mm[2]]
                }
                AnatomicalPlane::Sagittal => {
                    [self.geometry.spacing_mm[1], self.geometry.spacing_mm[2]]
                }
            },
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SliceView {
    plane: AnatomicalPlane,
    fixed_index: u32,
    dimensions: [u32; 2],
    volume_shape: [u32; 3],
    pixel_spacing_mm: [f64; 2],
}

impl SliceView {
    #[must_use]
    pub const fn plane(self) -> AnatomicalPlane {
        self.plane
    }

    #[must_use]
    pub const fn fixed_index(self) -> u32 {
        self.fixed_index
    }

    #[must_use]
    pub const fn dimensions(self) -> [u32; 2] {
        self.dimensions
    }

    /// Full volume extent — for stepping the fixed axis (PageUp/PageDown).
    #[must_use]
    pub const fn volume_shape(self) -> [u32; 3] {
        self.volume_shape
    }

    #[must_use]
    pub const fn pixel_spacing_mm(self) -> [f64; 2] {
        self.pixel_spacing_mm
    }

    #[must_use]
    pub const fn edge_labels(self) -> EdgeLabels {
        self.plane.edge_labels()
    }

    pub fn voxel_at(self, pixel: [u32; 2]) -> Result<[u32; 3], ViewError> {
        validate_pixel(self.dimensions, pixel)?;
        let inverted_vertical = self.dimensions[1] - 1 - pixel[1];
        Ok(match self.plane {
            AnatomicalPlane::Axial => [pixel[0], pixel[1], self.fixed_index],
            AnatomicalPlane::Coronal => [pixel[0], self.fixed_index, inverted_vertical],
            AnatomicalPlane::Sagittal => [self.fixed_index, pixel[0], inverted_vertical],
        })
    }

    pub fn voxel_at_fraction(self, fraction: [f32; 2]) -> Result<[u32; 3], ViewError> {
        if fraction.iter().any(|value| !value.is_finite()) {
            return Err(ViewError::NonFiniteScreenCoordinate);
        }
        let pixel = [
            fraction_to_pixel(fraction[0], self.dimensions[0]),
            fraction_to_pixel(fraction[1], self.dimensions[1]),
        ];
        self.voxel_at(pixel)
    }

    pub fn pixel_for_voxel(self, voxel: [u32; 3]) -> Result<[u32; 2], ViewError> {
        if voxel
            .into_iter()
            .zip(self.volume_shape)
            .any(|(index, extent)| index >= extent)
        {
            return Err(ViewError::VoxelOutOfBounds {
                voxel,
                shape: self.volume_shape,
            });
        }
        if voxel[self.plane.fixed_axis()] != self.fixed_index {
            return Err(ViewError::VoxelNotOnSlice {
                voxel,
                plane: self.plane,
                fixed_index: self.fixed_index,
            });
        }
        Ok(match self.plane {
            AnatomicalPlane::Axial => [voxel[0], voxel[1]],
            AnatomicalPlane::Coronal => [voxel[0], self.dimensions[1] - 1 - voxel[2]],
            AnatomicalPlane::Sagittal => [voxel[1], self.dimensions[1] - 1 - voxel[2]],
        })
    }

    pub fn linear_index_at(self, pixel: [u32; 2]) -> Result<usize, ViewError> {
        let voxel = self.voxel_at(pixel)?;
        Ok(linear_index(self.volume_shape, voxel))
    }

    pub fn extract<T: Copy>(self, values: &[T]) -> Result<Vec<T>, ViewError> {
        let expected = self
            .volume_shape
            .into_iter()
            .try_fold(1_usize, |count, extent| count.checked_mul(extent as usize));
        if expected != Some(values.len()) {
            return Err(ViewError::VolumeLength {
                expected: expected.unwrap_or(usize::MAX),
                actual: values.len(),
            });
        }
        let capacity = self.dimensions[0] as usize * self.dimensions[1] as usize;
        let mut output = Vec::with_capacity(capacity);
        for vertical in 0..self.dimensions[1] {
            for horizontal in 0..self.dimensions[0] {
                output.push(values[self.linear_index_at([horizontal, vertical])?]);
            }
        }
        Ok(output)
    }
}

#[derive(Debug, Error, PartialEq)]
pub enum ViewError {
    #[error("invalid grid geometry: {0}")]
    InvalidGeometry(String),
    #[error("grid direction {0:?} is not aligned to canonical DICOM LPS axes")]
    NotPatientAligned([f64; 9]),
    #[error("voxel {voxel:?} is outside grid shape {shape:?}")]
    VoxelOutOfBounds { voxel: [u32; 3], shape: [u32; 3] },
    #[error("pixel {pixel:?} is outside slice dimensions {dimensions:?}")]
    PixelOutOfBounds {
        pixel: [u32; 2],
        dimensions: [u32; 2],
    },
    #[error("voxel {voxel:?} is not on {plane:?} slice with fixed grid index {fixed_index}")]
    VoxelNotOnSlice {
        voxel: [u32; 3],
        plane: AnatomicalPlane,
        fixed_index: u32,
    },
    #[error("screen coordinate contains NaN or infinity")]
    NonFiniteScreenCoordinate,
    #[error("volume contains {actual} values; expected {expected}")]
    VolumeLength { expected: usize, actual: usize },
}

fn validate_voxel(geometry: &GridGeometry, voxel: [u32; 3]) -> Result<(), ViewError> {
    if voxel
        .into_iter()
        .zip(geometry.shape)
        .any(|(index, extent)| index >= extent)
    {
        return Err(ViewError::VoxelOutOfBounds {
            voxel,
            shape: geometry.shape,
        });
    }
    Ok(())
}

fn validate_pixel(dimensions: [u32; 2], pixel: [u32; 2]) -> Result<(), ViewError> {
    if pixel
        .into_iter()
        .zip(dimensions)
        .any(|(index, extent)| index >= extent)
    {
        return Err(ViewError::PixelOutOfBounds { pixel, dimensions });
    }
    Ok(())
}

fn fraction_to_pixel(fraction: f32, extent: u32) -> u32 {
    let scaled = fraction.clamp(0.0, 1.0) * extent as f32;
    (scaled.floor() as u32).min(extent - 1)
}

fn linear_index(shape: [u32; 3], voxel: [u32; 3]) -> usize {
    (voxel[2] as usize * shape[1] as usize + voxel[1] as usize) * shape[0] as usize
        + voxel[0] as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geometry() -> GridGeometry {
        GridGeometry {
            shape: [4, 6, 8],
            spacing_mm: [2.0, 3.0, 4.0],
            origin_mm: [-3.0, -7.5, -14.0],
            direction: IDENTITY_DIRECTION,
        }
    }

    #[test]
    fn maps_patient_aligned_view_edges_independently() {
        let grid = PatientAlignedGrid::new(&geometry()).expect("patient-aligned grid");
        let crosshair = Crosshair::new(&grid, [1, 2, 3]).expect("crosshair");

        let axial = grid
            .slice(AnatomicalPlane::Axial, crosshair)
            .expect("axial");
        assert_eq!(axial.dimensions(), [4, 6]);
        assert_eq!(axial.pixel_spacing_mm(), [2.0, 3.0]);
        assert_eq!(axial.voxel_at([0, 0]), Ok([0, 0, 3]));
        assert_eq!(axial.voxel_at([3, 5]), Ok([3, 5, 3]));
        assert_eq!(
            axial.edge_labels(),
            EdgeLabels {
                left: "R",
                right: "L",
                top: "A",
                bottom: "P"
            }
        );

        let coronal = grid
            .slice(AnatomicalPlane::Coronal, crosshair)
            .expect("coronal");
        assert_eq!(coronal.dimensions(), [4, 8]);
        assert_eq!(coronal.pixel_spacing_mm(), [2.0, 4.0]);
        assert_eq!(coronal.voxel_at([0, 0]), Ok([0, 2, 7]));
        assert_eq!(coronal.voxel_at([3, 7]), Ok([3, 2, 0]));
        assert_eq!(
            coronal.edge_labels(),
            EdgeLabels {
                left: "R",
                right: "L",
                top: "S",
                bottom: "I"
            }
        );

        let sagittal = grid
            .slice(AnatomicalPlane::Sagittal, crosshair)
            .expect("sagittal");
        assert_eq!(sagittal.dimensions(), [6, 8]);
        assert_eq!(sagittal.pixel_spacing_mm(), [3.0, 4.0]);
        assert_eq!(sagittal.voxel_at([0, 0]), Ok([1, 0, 7]));
        assert_eq!(sagittal.voxel_at([5, 7]), Ok([1, 5, 0]));
        assert_eq!(
            sagittal.edge_labels(),
            EdgeLabels {
                left: "A",
                right: "P",
                top: "S",
                bottom: "I"
            }
        );
    }

    #[test]
    fn crosshair_round_trips_through_every_view() {
        let grid = PatientAlignedGrid::new(&geometry()).expect("patient-aligned grid");
        let crosshair = Crosshair::new(&grid, [1, 2, 3]).expect("crosshair");
        for plane in AnatomicalPlane::ALL {
            let view = grid.slice(plane, crosshair).expect("view");
            let pixel = view
                .pixel_for_voxel(crosshair.voxel())
                .expect("crosshair pixel");
            assert_eq!(view.voxel_at(pixel), Ok(crosshair.voxel()), "{plane:?}");
        }
    }

    #[test]
    fn extraction_uses_columns_fastest_and_superior_at_top() {
        let grid = PatientAlignedGrid::new(&geometry()).expect("patient-aligned grid");
        let crosshair = Crosshair::new(&grid, [1, 2, 3]).expect("crosshair");
        let values: Vec<_> = (0..8)
            .flat_map(|slice| {
                (0..6)
                    .flat_map(move |row| (0..4).map(move |column| 100 * slice + 10 * row + column))
            })
            .collect();

        let coronal = grid
            .slice(AnatomicalPlane::Coronal, crosshair)
            .expect("coronal");
        let extracted = coronal.extract(&values).expect("extract");
        assert_eq!(extracted[0], 720);
        assert_eq!(extracted[3], 723);
        assert_eq!(extracted[7 * 4], 20);
        assert_eq!(extracted[7 * 4 + 3], 23);
    }

    #[test]
    fn click_selection_updates_all_linked_coordinates() {
        let grid = PatientAlignedGrid::new(&geometry()).expect("patient-aligned grid");
        let mut crosshair = Crosshair::centered(&grid);
        assert_eq!(crosshair.voxel(), [2, 3, 4]);
        let sagittal = grid
            .slice(AnatomicalPlane::Sagittal, crosshair)
            .expect("sagittal");
        crosshair
            .select_pixel(&grid, &sagittal, [1, 2])
            .expect("select pixel");
        assert_eq!(crosshair.voxel(), [2, 1, 5]);
        assert_eq!(crosshair.world_lps_mm(&grid), Ok([1.0, -4.5, 6.0]));
    }

    #[test]
    fn rejects_anatomical_labels_for_misaligned_grid() {
        let mut geometry = geometry();
        geometry.direction = [0.0, 1.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 1.0];
        assert!(matches!(
            PatientAlignedGrid::new(&geometry),
            Err(ViewError::NotPatientAligned(_))
        ));
    }

    #[test]
    fn normalized_edges_map_inside_the_last_pixel() {
        let grid = PatientAlignedGrid::new(&geometry()).expect("patient-aligned grid");
        let crosshair = Crosshair::new(&grid, [1, 2, 3]).expect("crosshair");
        let axial = grid
            .slice(AnatomicalPlane::Axial, crosshair)
            .expect("axial");
        assert_eq!(axial.voxel_at_fraction([0.0, 0.0]), Ok([0, 0, 3]));
        assert_eq!(axial.voxel_at_fraction([1.0, 1.0]), Ok([3, 5, 3]));
    }
}
