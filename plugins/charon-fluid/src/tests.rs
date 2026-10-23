use super::*;
fn domain(w: usize, h: usize, workers: usize) -> Fluid {
    let mut solid = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            solid[y * w + x] = x == 0 || y == 0 || x == w - 1 || y == h - 1;
        }
    }
    Fluid::new(
        Config {
            width: w,
            height: h,
            workers,
            density: [1., 0.8],
            drag: 0.8,
            viscosity: 0.5,
            pressure_cutoff: 0.01,
            sleep_tolerance: 1e-3,
        },
        solid,
    )
    .unwrap()
}
fn conserved(fluid: &Fluid, initial: [f64; 2]) {
    for (a, b) in fluid.volumes().into_iter().zip(initial) {
        assert!((a - b).abs() < 1e-7, "volume drift {a} vs {b}");
    }
    assert!(fluid
        .state
        .fraction
        .iter()
        .all(|v| v[0] >= -1e-10 && v[1] >= -1e-10 && v[0] + v[1] <= 1. + 1e-8));
}
#[test]
fn hydrostatic_oil_layer_stays_sharp_and_sleeps_then_wakes() {
    let mut fluid = domain(34, 34, 4);
    for y in 17..33 {
        for x in 1..33 {
            fluid.add(x, y, 0, 1.).unwrap();
        }
    }
    for y in 13..17 {
        for x in 1..33 {
            fluid.add(x, y, 1, 1.).unwrap();
        }
    }
    let initial = fluid.volumes();
    for _ in 0..100 {
        let s = fluid.step(1. / 60., [0., 120.]).unwrap();
        assert!(s.max_divergence < 1e-4, "{s:?}");
        conserved(&fluid, initial);
    }
    assert!(fluid.stats().sleeping, "{:?}", fluid.stats());
    assert!(fluid
        .state
        .fraction
        .iter()
        .all(|v| v[0] < 1e-7 || v[1] < 1e-7));
    let stats = fluid.step(1. / 60., [-900., 120.]).unwrap();
    assert!(!stats.sleeping && stats.max_speed > 1.);
}
#[test]
fn subcell_film_is_supported_by_the_floor_without_a_gravity_current() {
    let mut fluid = domain(26, 30, 2);
    for x in 1..25 {
        fluid.add(x, 28, 0, 0.2).unwrap();
    }
    let initial = fluid.volumes();
    for _ in 0..100 {
        let s = fluid.step(1. / 60., [0., 120.]).unwrap();
        assert!(s.max_speed < 0.01, "{s:?}");
        conserved(&fluid, initial);
    }
    assert!(fluid.stats().sleeping);
}
#[test]
fn sealed_full_tank_supports_pressure_without_an_air_reference() {
    let mut fluid = domain(18, 18, 2);
    for y in 1..17 {
        for x in 1..17 {
            fluid.add(x, y, 0, 1.).unwrap();
        }
    }
    let initial = fluid.volumes();
    for _ in 0..80 {
        let s = fluid.step(1. / 60., [40., 120.]).unwrap();
        conserved(&fluid, initial);
        assert!(s.max_divergence < 1e-4);
    }
    assert!(fluid.stats().sleeping);
}
#[test]
fn viscosity_dissipates_shear_energy_without_losing_full_tank_volume() {
    let mut a = domain(18, 18, 2);
    a.config.viscosity = 0.;
    a.config.drag = 0.;
    a.config.sleep_tolerance = 0.;
    for y in 1..17 {
        for x in 1..17 {
            a.add(x, y, 0, 1.).unwrap();
        }
    }
    for y in 1..17 {
        for x in 2..17 {
            a.state.u[y * 19 + x] = if y % 2 == 0 { 1. } else { -1. };
        }
    }
    let mut b = Fluid::new(
        Config {
            viscosity: 5.,
            ..a.config
        },
        a.solid.clone(),
    )
    .unwrap();
    b.state = a.state.clone();
    let initial = a.volumes();
    let free = a.step(1. / 60., [0., 0.]).unwrap();
    let viscous = b.step(1. / 60., [0., 0.]).unwrap();
    assert!(
        viscous.rms_speed < 0.9 * free.rms_speed,
        "{viscous:?} vs {free:?}"
    );
    conserved(&a, initial);
    conserved(&b, initial);
}
#[test]
fn dam_break_turns_a_corner_conserves_species_and_is_worker_independent() {
    let mut a = domain(42, 34, 4);
    // A submerged baffle makes the path non-rectangular.
    for y in 24..33 {
        a.set_solid(23, y, true).unwrap();
    }
    for y in 3..33 {
        for x in 1..12 {
            a.add(x, y, 0, 1.).unwrap();
        }
    }
    for y in 1..3 {
        for x in 1..12 {
            a.add(x, y, 1, 1.).unwrap();
        }
    }
    let mut b = Fluid::new(
        Config {
            workers: 1,
            ..a.config
        },
        a.solid.clone(),
    )
    .unwrap();
    b.state = a.state.clone();
    let initial = a.volumes();
    let mut transported = 0.;
    for tick in 0..180 {
        let s = a
            .step(1. / 60., [0., 120.])
            .unwrap_or_else(|e| panic!("tick={tick} {e:?}"));
        b.step(1. / 60., [0., 120.]).unwrap();
        assert_eq!(a.signature(), b.signature(), "worker order tick={tick}");
        assert!(s.max_divergence < 1e-3, "{s:?}");
        conserved(&a, initial);
        transported = (1..33)
            .flat_map(|y| (24..41).map(move |x| (x, y)))
            .map(|(x, y)| a.fraction(x, y).unwrap().iter().sum::<f64>())
            .sum();
    }
    assert!(
        transported > 5.,
        "water did not travel over the baffle: {transported}"
    );
}
#[test]
fn separated_droplet_falls_and_invalid_edits_do_not_destroy_liquid() {
    let mut fluid = domain(26, 40, 2);
    for y in 4..9 {
        for x in 10..15 {
            fluid.add(x, y, 0, 1.).unwrap();
        }
    }
    let initial = fluid.volumes();
    let before = fluid.signature();
    assert_eq!(fluid.set_solid(12, 5, true), Err(Error::Occupied));
    assert_eq!(fluid.add(12, 5, 1, 1.), Err(Error::Occupied));
    assert!(matches!(
        fluid.step(f64::NAN, [0., 120.]),
        Err(Error::InvalidInput)
    ));
    assert_eq!(fluid.signature(), before);
    for _ in 0..30 {
        fluid.step(1. / 60., [0., 120.]).unwrap();
        conserved(&fluid, initial);
    }
    let center = fluid
        .state
        .fraction
        .iter()
        .enumerate()
        .map(|(i, v)| (i / 26) as f64 * (v[0] + v[1]))
        .sum::<f64>()
        / initial[0];
    let t = 0.5;
    let drag = fluid.config.drag;
    let g = 120.;
    let expected = 6. + g / drag * (t - (1. - (-drag * t).exp()) / drag);
    assert!(
        (center - expected).abs() < 1.5,
        "falling liquid centroid {center}, analytic gravity/drag {expected}"
    );
    let before = fluid.signature();
    assert!(fluid.step(0.1, [1e9, 1e9]).is_err());
    assert_eq!(fluid.signature(), before, "failed tick did not roll back");
}
