use charon_core::{App, CoreError, Plugin, PluginResult, Step, World};

pub const COLS: i32 = 36;
pub const ROWS: i32 = 20;
pub const GEMS: [(i32, i32); 6] = [(5, 3), (15, 3), (29, 4), (7, 15), (23, 15), (32, 17)];

#[derive(Clone, Copy, Default)]
pub struct Input {
    pub dx: i32,
    pub dy: i32,
    pub reset: bool,
    pub pause: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Playing,
    Won,
    Lost,
}

pub struct Game {
    pub player: (i32, i32),
    pub gems: Vec<(i32, i32)>,
    pub health: u32,
    pub remaining: f64,
    pub paused: bool,
    pub status: Status,
    pub ticks: u64,
    movement: f64,
    invincible: f64,
}

impl Default for Game {
    fn default() -> Self {
        Self {
            player: (2, 2),
            gems: GEMS.to_vec(),
            health: 3,
            remaining: 60.0,
            paused: false,
            status: Status::Playing,
            ticks: 0,
            movement: 0.0,
            invincible: 0.0,
        }
    }
}

impl Game {
    pub fn hazard(&self) -> (i32, i32) {
        let phase = ((self.ticks / 12) % 52) as i32;
        (5 + if phase < 26 { phase } else { 51 - phase }, 10)
    }
}

pub fn wall(x: i32, y: i32) -> bool {
    x == 0
        || y == 0
        || x == COLS - 1
        || y == ROWS - 1
        || (x == 11 && (4..=16).contains(&y) && y != 10)
        || (x == 25 && (2..=14).contains(&y) && y != 7 && y != 10)
        || (y == 6 && (3..=8).contains(&x))
}

#[derive(Default)]
pub struct Score(pub u32);
pub struct Collected;
pub struct Restarted;
#[derive(Default)]
pub struct Trace(pub Vec<String>);

pub fn trace(world: &mut World, message: &str) {
    if let Some(log) = world.get_mut::<Trace>() {
        log.0.push(message.into());
    }
}

pub fn required<T: 'static>(world: &World) -> Result<&T, String> {
    world
        .get::<T>()
        .ok_or_else(|| format!("missing resource {}", std::any::type_name::<T>()))
}

pub struct GamePlugin;
impl Plugin for GamePlugin {
    fn name(&self) -> &'static str {
        "game"
    }
    fn init(&mut self, world: &mut World) -> PluginResult {
        world.insert(Game::default());
        world.insert(Input::default());
        trace(world, "game:init");
        Ok(())
    }
    fn fixed_update(&mut self, world: &mut World, step: Step) -> PluginResult {
        // Broadcast events belong to this producer and last until its next tick.
        world.clear_events::<Collected>();
        world.clear_events::<Restarted>();
        let input = *required::<Input>(world)?;
        let state = world.get_mut::<Input>().ok_or("input missing")?;
        state.reset = false;
        state.pause = false;
        if input.reset {
            world.insert(Game::default());
            world.send(Restarted);
        }
        let mut collected = false;
        {
            let game = world.get_mut::<Game>().ok_or("game missing")?;
            if input.pause {
                game.paused = !game.paused;
            }
            if game.paused || game.status != Status::Playing {
                return Ok(());
            }
            let dt = step.delta.as_secs_f64();
            game.ticks += 1;
            game.remaining = (game.remaining - dt).max(0.0);
            game.movement -= dt;
            game.invincible = (game.invincible - dt).max(0.0);
            if game.movement <= 0.0 && (input.dx != 0 || input.dy != 0) {
                let delta = if input.dx != 0 {
                    (input.dx.signum(), 0)
                } else {
                    (0, input.dy.signum())
                };
                let next = (game.player.0 + delta.0, game.player.1 + delta.1);
                if !wall(next.0, next.1) {
                    game.player = next;
                }
                game.movement = 0.09;
            }
            if let Some(index) = game.gems.iter().position(|&pos| pos == game.player) {
                game.gems.remove(index);
                collected = true;
            }
            if game.player == game.hazard() && game.invincible <= 0.0 {
                game.health -= 1;
                game.invincible = 1.0;
            }
            game.status = if game.health == 0 || game.remaining == 0.0 {
                Status::Lost
            } else if game.gems.is_empty() {
                Status::Won
            } else {
                Status::Playing
            };
        }
        if collected {
            world.send(Collected);
        }
        Ok(())
    }
    fn shutdown(&mut self, world: &mut World) -> PluginResult {
        world.remove::<Game>();
        world.remove::<Input>();
        world.clear_events::<Collected>();
        world.clear_events::<Restarted>();
        trace(world, "game:shutdown");
        Ok(())
    }
}

pub struct ScorePlugin;
impl Plugin for ScorePlugin {
    fn name(&self) -> &'static str {
        "score"
    }
    fn dependencies(&self) -> &[&'static str] {
        &["game"]
    }
    fn init(&mut self, world: &mut World) -> PluginResult {
        world.insert(Score::default());
        trace(world, "score:init");
        Ok(())
    }
    fn fixed_update(&mut self, world: &mut World, _: Step) -> PluginResult {
        let reset = !world.events::<Restarted>().is_empty();
        let points = world.events::<Collected>().len() as u32;
        let score = world.get_mut::<Score>().ok_or("score missing")?;
        if reset {
            score.0 = 0;
        }
        score.0 += points;
        Ok(())
    }
    fn shutdown(&mut self, world: &mut World) -> PluginResult {
        world.remove::<Score>();
        trace(world, "score:shutdown");
        Ok(())
    }
}

pub struct Glow(pub u32);
pub struct PulsePlugin;
impl Plugin for PulsePlugin {
    fn name(&self) -> &'static str {
        "pulse"
    }
    fn dependencies(&self) -> &[&'static str] {
        &["game"]
    }
    fn before(&self) -> &[&'static str] {
        &["view"]
    }
    fn init(&mut self, world: &mut World) -> PluginResult {
        world.insert(Glow(0));
        trace(world, "pulse:init");
        Ok(())
    }
    fn frame_update(&mut self, world: &mut World, step: Step) -> PluginResult {
        world.get_mut::<Glow>().ok_or("glow missing")?.0 = ((step.index / 2) % 20) as u32;
        Ok(())
    }
    fn shutdown(&mut self, world: &mut World) -> PluginResult {
        world.remove::<Glow>();
        trace(world, "pulse:shutdown");
        Ok(())
    }
}

pub fn make_app(effects: bool) -> Result<App, CoreError> {
    let mut app = App::new();
    app.world_mut().insert(Trace::default());
    // Intentionally register the view first to demonstrate dependency resolution.
    app.add_plugin(crate::view::ViewPlugin)?;
    app.add_plugin(ScorePlugin)?;
    if effects {
        app.add_plugin(PulsePlugin)?;
    }
    app.add_plugin(GamePlugin)?;
    app.start()?;
    Ok(app)
}

pub fn smoke_test() -> Result<(), Box<dyn std::error::Error>> {
    for effects in [false, true] {
        let mut app = make_app(effects)?;
        for position in GEMS {
            // Script the game state, using the same callbacks as the window host.
            app.world_mut()
                .get_mut::<Game>()
                .ok_or("game missing")?
                .player = position;
            app.tick()?;
            app.frame(std::time::Duration::from_millis(16))?;
        }
        if required::<Game>(app.world())?.status != Status::Won
            || required::<Score>(app.world())?.0 != 6
        {
            return Err("scripted collection did not win".into());
        }
        let canvas = required::<crate::view::Canvas>(app.world())?;
        if canvas.pixels.len() != crate::view::WIDTH * crate::view::HEIGHT {
            return Err("invalid canvas".into());
        }
        app.world_mut()
            .get_mut::<Input>()
            .ok_or("input missing")?
            .reset = true;
        app.tick()?;
        if required::<Score>(app.world())?.0 != 0 {
            return Err("restart did not reset score".into());
        }
        app.shutdown()?;
        let log = &required::<Trace>(app.world())?.0;
        println!("effects={effects}: {}", log.join(" -> "));
    }
    println!(
        "M1 smoke test passed: dependencies, events, frame output, restart, optional plugin, shutdown."
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripted_game_and_optional_plugin() {
        smoke_test().unwrap();
    }

    #[test]
    fn walls_pause_timeout_and_damage() {
        let mut app = make_app(false).unwrap();
        app.world_mut().get_mut::<Game>().unwrap().player = (1, 1);
        app.world_mut().get_mut::<Input>().unwrap().dx = -1;
        app.tick().unwrap();
        assert_eq!(required::<Game>(app.world()).unwrap().player, (1, 1));
        app.world_mut().get_mut::<Input>().unwrap().pause = true;
        app.tick().unwrap();
        let remaining = required::<Game>(app.world()).unwrap().remaining;
        app.tick().unwrap();
        assert_eq!(required::<Game>(app.world()).unwrap().remaining, remaining);
        app.world_mut().get_mut::<Input>().unwrap().pause = true;
        app.world_mut().get_mut::<Game>().unwrap().remaining = 0.001;
        app.tick().unwrap();
        assert_eq!(required::<Game>(app.world()).unwrap().status, Status::Lost);
        app.world_mut().get_mut::<Input>().unwrap().reset = true;
        app.world_mut().get_mut::<Input>().unwrap().dx = 0;
        app.tick().unwrap();
        let game = app.world_mut().get_mut::<Game>().unwrap();
        game.player = game.hazard();
        app.tick().unwrap();
        assert_eq!(required::<Game>(app.world()).unwrap().health, 2);
        app.tick().unwrap();
        assert_eq!(required::<Game>(app.world()).unwrap().health, 2);
    }
}
