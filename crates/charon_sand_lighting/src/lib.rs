//! Particle optics adapter. Uses snapshots, never physical active-chunk flags.
use charon_lighting::{Geometry, Optical, OpticalGrid};
use charon_physics::sim::{material::CellData, simulation::Snapshot};
pub fn rasterize(
    snapshot: &Snapshot,
    geometry: Geometry,
    origin: [f32; 2],
    particle_size: f32,
    mut properties: impl FnMut(CellData) -> Optical,
) -> Result<OpticalGrid, String> {
    if !particle_size.is_finite()
        || particle_size <= 0.
        || origin.iter().any(|v| !v.is_finite())
        || snapshot.width.checked_mul(snapshot.height) != Some(snapshot.cells.len())
    {
        return Err("invalid particle optical geometry".into());
    }
    let mut grid = OpticalGrid::new(geometry)?;
    for (i, &cell) in snapshot.cells.iter().enumerate() {
        let optical = properties(cell);
        if optical == Optical::AIR {
            continue;
        }
        grid.raster_rect(
            [
                origin[0] + (i % snapshot.width) as f32 * particle_size,
                origin[1] + (i / snapshot.width) as f32 * particle_size,
                particle_size,
                particle_size,
            ],
            optical,
        )?;
    }
    Ok(grid)
}
