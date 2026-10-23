//! Versioned development trace. File IO is opt-in and is not a performance mode.
use crate::game::{Command, Game};
use charon_runtime::Simulation;
use charon_sand::Command as SandCommand;
use std::{
    fs::{File, OpenOptions},
    io::{BufRead, BufReader, BufWriter, Write},
    path::Path,
};

pub struct Recorder {
    writer: BufWriter<File>,
    tick: u64,
}
impl Recorder {
    pub fn with_backend(
        path: &Path,
        width: usize,
        height: usize,
        workers: usize,
        play: bool,
        materials: u64,
        backend: &str,
    ) -> std::io::Result<Self> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let mut writer =
            BufWriter::new(OpenOptions::new().write(true).create_new(true).open(path)?);
        if backend == "cpu" {
            writeln!(
                writer,
                "charon-input-v4 {width} {height} {workers} {} {materials:016x}",
                u8::from(play)
            )?;
        } else {
            writeln!(
                writer,
                "charon-input-v5 {width} {height} {workers} {} {materials:016x} {backend}",
                u8::from(play)
            )?;
        }
        writer.flush()?;
        Ok(Self { writer, tick: 0 })
    }
    pub fn append(&mut self, signature: u64, commands: &[Command]) -> std::io::Result<()> {
        let commands = commands.iter().map(encode).collect::<Vec<_>>().join(";");
        writeln!(self.writer, "{}|{signature:016x}|{commands}", self.tick)?;
        self.writer.flush()?;
        self.tick += 1;
        Ok(())
    }
}
fn encode(command: &Command) -> String {
    match command {
        Command::CupScene => "cup".into(),
        Command::DamScene => "dam".into(),
        Command::AddOil => "oil".into(),
        Command::Explore(seed) => format!("explore:{seed}"),
        Command::NewRun(seed) => format!("run:{seed}"),
        Command::StartWorld(seed) => format!("world:{seed}"),
        Command::LoadRegion { origin, cells } => {
            let mut runs = Vec::new();
            let mut start = 0;
            while start < cells.len() {
                let mut end = start + 1;
                while end < cells.len() && end - start < 65535 && cells[end] == cells[start] {
                    end += 1;
                }
                runs.extend(((end - start) as u16).to_le_bytes());
                runs.push(cells[start]);
                start = end;
            }
            let packed = runs.len() < cells.len();
            let bytes = if packed {
                runs.as_slice()
            } else {
                cells.as_ref()
            };
            let mut hex = Vec::with_capacity(bytes.len() * 2);
            const DIGITS: &[u8] = b"0123456789abcdef";
            for &cell in bytes {
                hex.push(DIGITS[(cell >> 4) as usize]);
                hex.push(DIGITS[(cell & 15) as usize]);
            }
            format!(
                "{}:{}:{}:{}",
                if packed { "region-rle" } else { "region" },
                origin[0],
                origin[1],
                String::from_utf8(hex).expect("hex ASCII")
            )
        }
        Command::Begin => "begin".into(),
        Command::Terrain(SandCommand::Paint { x, y, material }) => {
            format!("paint:{x}:{y}:{material}")
        }
        Command::Terrain(SandCommand::Clear) => "clear".into(),
        Command::Terrain(SandCommand::FillRect {
            x,
            y,
            width,
            height,
            material,
        }) => format!("rect:{x}:{y}:{width}:{height}:{material}"),
        Command::PlaceImage([x, y]) => format!("image:{:08x}:{:08x}", x.to_bits(), y.to_bits()),
        Command::EraseImage([x, y]) => {
            format!("erase-image:{:08x}:{:08x}", x.to_bits(), y.to_bits())
        }
        Command::Left(down) => format!("left:{}", u8::from(*down)),
        Command::Right(down) => format!("right:{}", u8::from(*down)),
        Command::Dig(down) => format!("dig:{}", u8::from(*down)),
        Command::Jump => "jump".into(),
        Command::Fire => "fire".into(),
        Command::LobAt([x, y]) => format!("lob-at:{:08x}:{:08x}", x.to_bits(), y.to_bits()),
        Command::FireAt([x, y]) => format!("fire-at:{:08x}:{:08x}", x.to_bits(), y.to_bits()),
        Command::ToggleSand => "sand".into(),
        Command::Pause => "pause".into(),
        Command::Step => "step".into(),
        Command::Inspect(None) => "inspect:none".into(),
        Command::Inspect(Some((x, y))) => format!("inspect:{x}:{y}"),
    }
}
fn decode(text: &str) -> Result<Command, String> {
    let fields: Vec<_> = text.split(':').collect();
    let bad = || {
        format!(
            "invalid input command: {}",
            text.chars().take(128).collect::<String>()
        )
    };
    Ok(match fields.as_slice() {
        ["paint", x, y, m] => Command::Terrain(SandCommand::Paint {
            x: x.parse().map_err(|_| bad())?,
            y: y.parse().map_err(|_| bad())?,
            material: m.parse().map_err(|_| bad())?,
        }),
        ["rect", x, y, w, h, m] => Command::Terrain(SandCommand::FillRect {
            x: x.parse().map_err(|_| bad())?,
            y: y.parse().map_err(|_| bad())?,
            width: w.parse().map_err(|_| bad())?,
            height: h.parse().map_err(|_| bad())?,
            material: m.parse().map_err(|_| bad())?,
        }),
        [kind @ ("image" | "erase-image" | "fire-at" | "lob-at"), x, y] => {
            let position = [x, y].map(|v| {
                u32::from_str_radix(v, 16)
                    .map(f32::from_bits)
                    .map_err(|_| bad())
            });
            let point = [
                position[0].as_ref().copied().map_err(|_| bad())?,
                position[1].as_ref().copied().map_err(|_| bad())?,
            ];
            if !point.into_iter().all(f32::is_finite) {
                return Err(bad());
            }
            match *kind {
                "image" => Command::PlaceImage(point),
                "fire-at" => Command::FireAt(point),
                "lob-at" => Command::LobAt(point),
                _ => Command::EraseImage(point),
            }
        }
        ["explore", seed] => Command::Explore(seed.parse().map_err(|_| bad())?),
        ["run", seed] => Command::NewRun(seed.parse().map_err(|_| bad())?),
        ["world", seed] => Command::StartWorld(seed.parse().map_err(|_| bad())?),
        [kind @ ("region" | "region-rle"), x, y, hex] => {
            if hex.len() > 1920 * 1080 * 2 || hex.len() % 2 != 0 {
                return Err(bad());
            }
            fn digit(c: u8) -> Option<u8> {
                match c {
                    b'0'..=b'9' => Some(c - b'0'),
                    b'a'..=b'f' => Some(c - b'a' + 10),
                    _ => None,
                }
            }
            let cells: Result<Vec<u8>, String> = hex
                .as_bytes()
                .as_chunks::<2>()
                .0
                .iter()
                .map(|p| Ok((digit(p[0]).ok_or_else(bad)? << 4) | digit(p[1]).ok_or_else(bad)?))
                .collect();
            let bytes = cells?;
            let cells = if *kind == "region-rle" {
                let (runs, extra) = bytes.as_chunks::<3>();
                if !extra.is_empty() {
                    return Err(bad());
                }
                let mut expanded = Vec::new();
                for &[lo, hi, cell] in runs {
                    let count = u16::from_le_bytes([lo, hi]) as usize;
                    if count == 0 || expanded.len() + count > 1920 * 1080 {
                        return Err(bad());
                    }
                    expanded.resize(expanded.len() + count, cell);
                }
                expanded
            } else {
                bytes
            };
            Command::LoadRegion {
                origin: [x.parse().map_err(|_| bad())?, y.parse().map_err(|_| bad())?],
                cells: cells.into(),
            }
        }
        ["begin"] => Command::Begin,
        ["cup"] => Command::CupScene,
        ["dam"] => Command::DamScene,
        ["oil"] => Command::AddOil,
        ["clear"] => Command::Terrain(SandCommand::Clear),
        ["jump"] => Command::Jump,
        ["fire"] => Command::Fire,
        ["sand"] => Command::ToggleSand,
        ["pause"] => Command::Pause,
        ["step"] => Command::Step,
        ["inspect", "none"] => Command::Inspect(None),
        ["inspect", x, y] => Command::Inspect(Some((
            x.parse().map_err(|_| bad())?,
            y.parse().map_err(|_| bad())?,
        ))),
        [key, value @ ("0" | "1")] => match *key {
            "left" => Command::Left(*value == "1"),
            "right" => Command::Right(*value == "1"),
            "dig" => Command::Dig(*value == "1"),
            _ => return Err(bad()),
        },
        _ => return Err(bad()),
    })
}
pub fn replay(path: &Path, workers: usize) -> Result<u64, String> {
    replay_game(path, workers).map(|(_, ticks)| ticks)
}
fn replay_game(path: &Path, workers: usize) -> Result<(Game, u64), String> {
    let mut lines = BufReader::new(File::open(path).map_err(|e| e.to_string())?).lines();
    let header = lines
        .next()
        .ok_or("missing input header")?
        .map_err(|e| e.to_string())?;
    let fields: Vec<_> = header.split_whitespace().collect();
    let legacy = fields.first() == Some(&"charon-input-v1");
    if !(legacy && fields.len() == 5
        || fields.first() == Some(&"charon-input-v5") && fields.len() == 7
        || matches!(
            fields.first(),
            Some(&"charon-input-v2" | &"charon-input-v3" | &"charon-input-v4")
        ) && fields.len() == 6)
        || !["0", "1"].contains(&fields[4])
    {
        return Err("unsupported input header".into());
    }
    let width = fields[1].parse().map_err(|_| "invalid width")?;
    let height = fields[2].parse().map_err(|_| "invalid height")?;
    let old = legacy || fields[0] == "charon-input-v2";
    let materials = if old {
        crate::materials::legacy_palette()
    } else {
        crate::materials::palette()
    };
    let expected_materials = if old {
        materials.legacy_fingerprint()
    } else {
        materials.fingerprint()
    };
    let mut game = Game::with_materials(width, height, workers, fields[4] == "1", materials)?;
    if !legacy && u64::from_str_radix(fields[5], 16).ok() != Some(expected_materials) {
        return Err(
            "material definitions differ from input trace; use the matching game version".into(),
        );
    }
    if fields[0] == "charon-input-v5" {
        let gpu = std::sync::Arc::new(charon_fluid::gpu::GpuContext::new()?);
        if fields[6] != gpu.identity() {
            return Err(
                "GPU trace backend/driver differs; exact replay requires the recorded adapter"
                    .into(),
            );
        }
        game.use_fluid_gpu(gpu);
    }
    let cfd_trace = matches!(fields[0], "charon-input-v4" | "charon-input-v5");
    let mut ticks = 0;
    for line in lines {
        let line = line.map_err(|e| e.to_string())?;
        let fields: Vec<_> = line.split('|').collect();
        if fields.len() != 3 || fields[0].parse::<u64>().ok() != Some(ticks) {
            return Err(format!("invalid record at tick {ticks}"));
        }
        let expected = u64::from_str_radix(fields[1], 16)
            .map_err(|_| format!("invalid checksum at tick {ticks}"))?;
        let commands = if fields[2].is_empty() {
            vec![]
        } else {
            fields[2].split(';').map(decode).collect::<Result<_, _>>()?
        };
        for command in &commands {
            if !cfd_trace
                && matches!(
                    command,
                    Command::CupScene | Command::DamScene | Command::AddOil
                )
            {
                return Err("cup physics changed; record a v4 trace with the CFD solver".into());
            }
            if let Command::Terrain(
                SandCommand::Paint { material, .. } | SandCommand::FillRect { material, .. },
            ) = command
            {
                if !game.has_material(*material) || legacy && *material > 2 {
                    return Err(format!("unknown material {material} at tick {ticks}"));
                }
            }
        }
        game.step(commands);
        let actual = game.signature();
        if actual != expected {
            return Err(format!(
                "state mismatch at tick {ticks}: expected {expected:016x}, got {actual:016x}"
            ));
        }
        ticks += 1;
    }
    if ticks == 0 {
        return Err("input trace contains no ticks".into());
    }
    Ok((game, ticks))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recorded_sea_reaches_rest_without_losing_water() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/media/sea-linux.trace");
        let (mut game, _) = replay_game(&path, 1).unwrap();
        let water = game.step(vec![]).terrain.liquid;
        for _ in 0..2400 {
            let frame = game.step(vec![]);
            assert_eq!(frame.terrain.liquid, water);
            if frame.terrain.active_tiles == 0 {
                let signature = game.signature();
                for _ in 0..5 {
                    let idle = game.step(vec![]);
                    assert_eq!(
                        (
                            idle.terrain.active_tiles,
                            idle.terrain.visited_cells,
                            idle.terrain.converted_cells
                        ),
                        (0, 0, 0)
                    );
                    assert_eq!(game.signature(), signature);
                }
                return;
            }
        }
        panic!("recorded sea never settled");
    }
    #[test]
    fn recorded_inputs_replay_across_workers_and_detect_corruption() {
        let path =
            std::env::temp_dir().join(format!("charon-input-test-{}.trace", std::process::id()));
        // create_new protects existing user files; this unique test path is never overwritten.
        let mut game = Game::new(160, 120, 4, true).unwrap();
        game.record_to(&path).unwrap();
        for tick in 0..120 {
            let commands = match tick {
                1 => vec![Command::Right(true)],
                10 => vec![Command::Right(false), Command::Jump, Command::Fire],
                35 => vec![Command::ToggleSand, Command::Pause],
                38 => vec![Command::Jump],
                40 => vec![Command::Step, Command::Step],
                45 => vec![Command::Pause],
                52 => vec![Command::PlaceImage([45.25, 30.75])],
                53 => vec![Command::Terrain(SandCommand::FillRect {
                    x: 80,
                    y: 32,
                    width: 8,
                    height: 8,
                    material: 2,
                })],
                54 => vec![Command::Terrain(SandCommand::Paint {
                    x: 100,
                    y: 20,
                    material: crate::materials::ASH,
                })],
                55 => vec![Command::Inspect(Some((20, 30)))],
                56 => vec![Command::Inspect(None)],
                50 => vec![Command::Dig(true)],
                100 => vec![Command::Terrain(SandCommand::Clear)],
                105 => vec![Command::CupScene],
                106 => vec![Command::AddOil],
                107 => vec![Command::Right(true)],
                113 => vec![Command::Right(false), Command::Left(true)],
                117 => vec![Command::DamScene],
                _ => vec![],
            };
            game.step(commands);
        }
        drop(game);
        assert_eq!(replay(&path, 1).unwrap(), 120);
        let text = std::fs::read_to_string(&path).unwrap();
        let invalid_palette = text.replacen(
            &format!(
                "{:016x}",
                Game::new(160, 120, 1, true).unwrap().material_fingerprint()
            ),
            "0000000000000000",
            1,
        );
        std::fs::write(&path, invalid_palette).unwrap();
        assert!(replay(&path, 1)
            .unwrap_err()
            .contains("material definitions"));
        let mut lines: Vec<_> = text.lines().map(str::to_owned).collect();
        let parts: Vec<_> = lines[31].split('|').collect();
        lines[31] = format!("{}|0000000000000000|{}", parts[0], parts[2]);
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();
        assert!(replay(&path, 1).unwrap_err().contains("tick 30"));
        std::fs::write(&path, "charon-input-v1 160 120 4 1\n1|abc|jump\n").unwrap();
        assert!(replay(&path, 1).unwrap_err().contains("invalid record"));
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn generated_map_and_retry_replay_with_other_worker_counts() {
        let path =
            std::env::temp_dir().join(format!("charon-cave-test-{}.trace", std::process::id()));
        let mut game = Game::new(480, 270, 4, true).unwrap();
        game.record_to(&path).unwrap();
        for tick in 0..100 {
            let commands = match tick {
                0 => vec![Command::Explore(42)],
                1 => vec![Command::Right(true)],
                15 => vec![Command::Right(false), Command::Dig(true)],
                25 => vec![Command::Dig(false), Command::FireAt([250.25, 150.5])],
                30 => vec![Command::Terrain(SandCommand::FillRect {
                    x: 120,
                    y: 160,
                    width: 8,
                    height: 8,
                    material: 2,
                })],
                40 => vec![Command::Pause],
                41 => vec![Command::Terrain(SandCommand::Clear)],
                42 => vec![Command::Step, Command::Pause],
                60 => vec![Command::Explore(u64::MAX)],
                _ => vec![],
            };
            game.step(commands);
        }
        drop(game);
        assert_eq!(replay(&path, 1).unwrap(), 100);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn combat_lifecycle_replays_death_and_retry_across_workers() {
        use crate::combat::Phase;
        let path =
            std::env::temp_dir().join(format!("charon-combat-test-{}.trace", std::process::id()));
        let mut game = Game::new(480, 270, 4, true).unwrap();
        game.record_to(&path).unwrap();
        let mut saw_defeat = false;
        for tick in 0..430 {
            let commands = match tick {
                0 => vec![Command::NewRun(42)],
                1 => vec![Command::Begin],
                400 => vec![Command::Terrain(SandCommand::Clear)],
                405 => vec![Command::LobAt([291.5, 154.])],
                _ => vec![],
            };
            let frame = game.step(commands);
            saw_defeat |= frame.combat.as_ref().unwrap().phase == Phase::Defeated;
        }
        assert!(saw_defeat);
        drop(game);
        assert_eq!(replay(&path, 1).unwrap(), 430);
        std::fs::remove_file(path).unwrap();
    }
}
