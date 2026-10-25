//! Optional particle snapshot adapter. Rendering never imports simulation types.
use charon_assets::Image;
use charon_physics::sim::{material::Cell, simulation::Snapshot};
use charon_render::{DrawFrame, PassId, Quad, TextureId};
pub fn image(snapshot: &Snapshot) -> Result<Image, String> {
    let mut rgba = Vec::with_capacity(snapshot.cells.len() * 4);
    for cell in &snapshot.cells {
        let mut color = snapshot
            .palette
            .get(cell.kind.id() as usize)
            .ok_or("unknown particle material")?
            .color;
        if cell.kind == Cell::Air {
            color[3] = 0;
        }
        rgba.extend_from_slice(&color);
    }
    Image::new(
        snapshot.width.try_into().map_err(|_| "width overflow")?,
        snapshot.height.try_into().map_err(|_| "height overflow")?,
        rgba,
    )
}
pub fn append(frame: &mut DrawFrame, texture: TextureId, rect: [f32; 4], layer: i32, pass: PassId) {
    frame.quads.push(Quad {
        rect,
        uv: [0., 0., 1., 1.],
        tint: [1.; 4],
        texture,
        layer,
        pass,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use charon_physics::sim::{
        grid::Grid,
        material::CellData,
        rules::{MaterialDefinition, Movement},
        simulation::Simulation,
    };
    #[test]
    fn registered_palette_and_transparency_are_preserved() {
        let mut grid = Grid::new(8, 8);
        let id = grid
            .materials
            .register(MaterialDefinition::new(
                "Cloud",
                Movement::Gas,
                0.7,
                [22, 33, 44, 120],
            ))
            .unwrap();
        grid.set(3, 3, CellData::new(id, 0));
        let image = image(&Simulation::from_grid(grid, 0).snapshot()).unwrap();
        assert_eq!(image.rgba[3], 0);
        let i = (3 * 8 + 3) * 4;
        assert_eq!(&image.rgba[i..i + 4], &[22, 33, 44, 120]);
    }
}
