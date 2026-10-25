mod app;
mod input;
mod render;
mod ui;

use app::{App, AppOptions};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::WindowId;

struct AppState {
    app: Option<App>,
    options: AppOptions,
}

impl ApplicationHandler for AppState {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.app.is_none() {
            // `AppOptions` is `Default + non-Copy`, so we move it out by swap.
            let opts = std::mem::take(&mut self.options);
            self.app = Some(App::new(event_loop, opts));
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        if let Some(app) = &mut self.app {
            app.handle_event(event_loop, event);
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(app) = &self.app {
            app.window.request_redraw();
        }
    }
}

fn parse_args() -> AppOptions {
    let mut opts = AppOptions::default();
    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--scenario" | "-s" => {
                if let Some(v) = args.get(i + 1).and_then(|s| s.parse::<usize>().ok()) {
                    opts.initial_scenario = v;
                    i += 1;
                }
            }
            "--help" | "-h" => {
                println!("sandengine - 2D falling-sand cellular automaton");
                println!();
                println!("Usage: sandengine [--scenario N]");
                println!();
                println!("Options:");
                println!("  --scenario N, -s N   Start with scenario index N (0..18)");
                println!("  --help, -h           Show this message");
                println!();
                println!("Scenarios:");
                for (i, name) in sandengine::sim::scenarios::NAMES.iter().enumerate() {
                    println!("  {i:>2}  {name}");
                }
                std::process::exit(0);
            }
            _ => {}
        }
        i += 1;
    }
    opts
}

fn main() {
    let options = parse_args();
    let event_loop = EventLoop::new().unwrap();
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut state = AppState { app: None, options };
    event_loop.run_app(&mut state).unwrap();
}
