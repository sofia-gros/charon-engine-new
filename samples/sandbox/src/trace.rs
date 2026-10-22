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
    ) -> std::io::Result<Self> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let mut writer =
            BufWriter::new(OpenOptions::new().write(true).create_new(true).open(path)?);
        writeln!(
            writer,
            "charon-input-v1 {width} {height} {workers} {}",
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
        Command::Left(down) => format!("left:{}", u8::from(*down)),
        Command::Right(down) => format!("right:{}", u8::from(*down)),
        Command::Dig(down) => format!("dig:{}", u8::from(*down)),
        Command::Jump => "jump".into(),
        Command::ToggleSand => "sand".into(),
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
        ["clear"] => Command::Terrain(SandCommand::Clear),
        ["jump"] => Command::Jump,
        ["sand"] => Command::ToggleSand,
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
    if fields.len() != 5 || fields[0] != "charon-input-v1" || !["0", "1"].contains(&fields[4]) {
        return Err("unsupported input header".into());
    }
    let width = fields[1].parse().map_err(|_| "invalid width")?;
    let height = fields[2].parse().map_err(|_| "invalid height")?;
    let mut game = Game::new(width, height, workers, fields[4] == "1")?;
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
                10 => vec![Command::Right(false), Command::Jump],
                35 => vec![Command::ToggleSand],
                50 => vec![Command::Dig(true)],
                100 => vec![Command::Terrain(SandCommand::Clear)],
                _ => vec![],
            };
            game.step(commands);
        }
        drop(game);
        assert_eq!(replay(&path, 1).unwrap(), 120);
        let text = std::fs::read_to_string(&path).unwrap();
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
