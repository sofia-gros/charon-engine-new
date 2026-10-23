/// Placement is chosen per object or tool, independently of camera and drawing.
#[derive(Clone, Copy, Debug)]
pub enum Placement {
    Free,
    Grid { origin: [f32; 2], cell: [f32; 2] },
}
impl Placement {
    pub fn position(self, requested: [f32; 2]) -> Result<[f32; 2], &'static str> {
        if !requested.into_iter().all(f32::is_finite) {
            return Err("placement position must be finite");
        }
        match self {
            Self::Free => Ok(requested),
            Self::Grid { origin, cell } => {
                if !origin.into_iter().chain(cell).all(f32::is_finite)
                    || cell.iter().any(|v| *v <= 0.)
                {
                    return Err("grid origin must be finite and cell size positive");
                }
                let placed = std::array::from_fn(|i| {
                    origin[i] + ((requested[i] - origin[i]) / cell[i]).floor() * cell[i]
                });
                if !placed.into_iter().all(f32::is_finite) {
                    return Err("grid placement out of range");
                }
                Ok(placed)
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn blocks_snap_in_world_coordinates_while_images_keep_fractional_positions() {
        let point = [101.25, 69.75];
        assert_eq!(Placement::Free.position(point).unwrap(), point);
        assert_eq!(
            Placement::Grid {
                origin: [0., 0.],
                cell: [8., 8.]
            }
            .position(point)
            .unwrap(),
            [96., 64.]
        );
        assert_eq!(
            Placement::Grid {
                origin: [2., 3.],
                cell: [8., 8.]
            }
            .position([-0.25, 4.5])
            .unwrap(),
            [-6., 3.]
        );
        assert!(Placement::Grid {
            origin: [0., 0.],
            cell: [0., 8.]
        }
        .position(point)
        .is_err());
    }
}
