use charon_core::App;
use std::time::Duration;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = App::new();
    app.start()?;
    let report = app.advance(Duration::from_millis(20))?;
    app.shutdown()?;
    println!(
        "Empty Core: {} fixed step(s), {:?}",
        report.fixed_steps,
        app.state()
    );
    Ok(())
}
