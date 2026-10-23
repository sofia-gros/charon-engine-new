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
    pub fn new(
        path: &Path,
        width: usize,
        height: usize,
        workers: usize,
        play: bool,
        materials: u64,
    ) -> std::io::Result<Self> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let mut writer =
            BufWriter::new(OpenOptions::new().write(true).create_new(true).open(path)?);
        writeln!(
            writer,
            "charon-input-v2 {width} {height} {workers} {} {materials:016x}",
            u8::from(play)
        )?;
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
        Command::ToggleSand => "sand".into(),
        Command::Pause => "pause".into(),
        Command::Step => "step".into(),
        Command::Inspect(None) => "inspect:none".into(),
        Command::Inspect(Some((x, y))) => format!("inspect:{x}:{y}"),
    }
}
fn decode(text: &str) -> Result<Command, String> {
    let fields: Vec<_> = text.split(':').collect();
    let bad = || format!("invalid input command: {text}");
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
        [kind @ ("image" | "erase-image"), x, y] => {
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
            if *kind == "image" {
                Command::PlaceImage(point)
            } else {
                Command::EraseImage(point)
            }
        }
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
    let mut lines = BufReader::new(File::open(path).map_err(|e| e.to_string())?).lines();
    let header = lines
        .next()
        .ok_or("missing input header")?
        .map_err(|e| e.to_string())?;
    let fields: Vec<_> = header.split_whitespace().collect();
    let legacy = fields.first() == Some(&"charon-input-v1");
    if !(legacy && fields.len() == 5
        || fields.first() == Some(&"charon-input-v2") && fields.len() == 6)
        || !["0", "1"].contains(&fields[4])
    {
        return Err("unsupported input header".into());
    }
    let width = fields[1].parse().map_err(|_| "invalid width")?;
    let height = fields[2].parse().map_err(|_| "invalid height")?;
    let mut game = Game::new(width, height, workers, fields[4] == "1")?;
    if !legacy && u64::from_str_radix(fields[5], 16).ok() != Some(game.material_fingerprint()) {
        return Err(
            "material definitions differ from input trace; use the matching game version".into(),
        );
    }
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
    Ok(ticks)
}
#[cfg(test)]
mod tests {
    use super::*;
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
}
