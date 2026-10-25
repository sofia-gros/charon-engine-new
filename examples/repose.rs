#[path = "../tests/support/repose_measurement.rs"]
mod measurement;
fn main() {
    for ground in [
        measurement::Ground::Flat,
        measurement::Ground::Slope,
        measurement::Ground::Step,
    ] {
        for angle in [32.0, 40.0, 45.0, 60.0] {
            let m = measurement::measure(angle, ground, 42);
            println!(
                "ground={ground:?} target={} left={:.2} right={:.2} ticks={} sand={}/{} settled={}",
                m.target,
                m.left_degrees,
                m.right_degrees,
                m.ticks,
                m.sand_after,
                m.sand_before,
                m.settled
            );
        }
    }
}
