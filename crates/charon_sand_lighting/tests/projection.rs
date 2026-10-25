use charon_lighting::*;
use charon_physics::sim::{
    grid::Grid,
    material::CellData,
    rules::{MaterialDefinition, Movement},
    simulation::Simulation,
};
#[test]
fn custom_material_emission_and_particle_motion_need_no_active_chunk_contract() {
    let mut grid = Grid::new(16, 16);
    let id = grid
        .materials
        .register(MaterialDefinition::new(
            "Emitting cloud",
            Movement::Gas,
            0.8,
            [255; 4],
        ))
        .unwrap();
    grid.set(5, 7, CellData::new(id, 0));
    let mut simulation = Simulation::from_grid(grid, 3);
    let g = Geometry {
        width: 8,
        height: 8,
        origin: [0., 0.],
        cell_size: 2.,
    };
    let project = |snapshot: &charon_physics::sim::simulation::Snapshot| {
        charon_sand_lighting::rasterize(snapshot, g, [0., 0.], 1., |cell| {
            if cell.kind == id {
                Optical {
                    emission_color: [0.2, 0.5, 1.],
                    emission_strength: 1.,
                    ..Optical::AIR
                }
            } else {
                Optical::AIR
            }
        })
        .unwrap()
    };
    let first = project(&simulation.snapshot());
    assert!(first.cells().iter().any(|o| o.emission_strength > 0.));
    for _ in 0..8 {
        simulation.step();
    }
    let next = project(&simulation.snapshot());
    assert_ne!(first, next);
}
