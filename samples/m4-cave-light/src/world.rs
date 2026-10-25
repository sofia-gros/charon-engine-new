use charon_tilemap::{TileDefinition, TileId, Tilemap};
pub fn map() -> Tilemap {
    let mut map = Tilemap::new(40, 24, [8, 8]).unwrap();
    for (name, index) in [
        ("Stone", 0),
        ("Sand block", 1),
        ("Water block", 2),
        ("Luminous ore", 3),
    ] {
        map.register(TileDefinition {
            name: name.into(),
            solid: true,
            atlas_index: index,
            tint: [1.; 4],
        })
        .unwrap();
    }
    for y in 0..24 {
        for x in 0..40 {
            let underground = y >= 7;
            let cave = (3..37).contains(&x) && (9..22).contains(&y);
            let shaft = (7..9).contains(&x) && (5..12).contains(&y);
            let mut tile = if underground && !cave && !shaft {
                Some(TileId(0))
            } else {
                None
            };
            if x == 0 || x == 39 || y == 23 || (x == 19 && (9..22).contains(&y)) {
                tile = Some(TileId(0));
            }
            if (12..15).contains(&x) && (10..14).contains(&y) {
                tile = Some(TileId(1));
            }
            if (24..27).contains(&x) && (10..14).contains(&y) {
                tile = Some(TileId(2));
            }
            if (x, y) == (31, 16) {
                tile = Some(TileId(3));
            }
            if let Some(id) = tile {
                map.set(x, y, Some(id)).unwrap();
            }
        }
    }
    map.drain_changes().for_each(drop);
    map
}
pub fn smoke() -> Result<(), Box<dyn std::error::Error>> {
    use charon_m3_sample::runtime::{Edit, Runtime};
    use std::time::{Duration, Instant};
    for hz in [30, 60] {
        for coupled in [false, true] {
            if coupled && !cfg!(feature = "bridge") {
                continue;
            }
            let mut runtime = Runtime::start_with_map(hz, coupled, map())?;
            runtime.edits.try_send(Edit::Break(31, 16))?;
            let end = Instant::now() + Duration::from_secs(5);
            loop {
                runtime.check()?;
                if runtime
                    .shared
                    .view
                    .lock()
                    .unwrap()
                    .map
                    .get(31, 16)
                    .is_none()
                {
                    break;
                }
                if Instant::now() > end {
                    return Err("cave edit timed out".into());
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            runtime.shutdown()?;
            println!("M4 cave game144 / compute{hz}, bridge={coupled}: world edit and shutdown OK");
        }
    }
    Ok(())
}
