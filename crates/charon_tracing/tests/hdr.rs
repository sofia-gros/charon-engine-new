use charon_tracing::{charon_lighting::*, hdr, *};
fn scene(width: u32, height: u32, size: f32) -> TraceScene {
    TraceScene::new(
        Scene::new(
            Geometry {
                width,
                height,
                origin: [0.; 2],
                cell_size: size,
            },
            Settings {
                sky: [0.; 3],
                ambient: [0.; 3],
                max_distance: 12.,
                ao_strength: 0.,
                ..Settings::default()
            },
        )
        .unwrap()
        .prepare(),
    )
    .unwrap()
}
#[test]
fn hdr_emission_is_independent_of_dark_albedo_and_not_clipped() {
    let mut s = scene(4, 4, 1.);
    s.materials[5] = Material {
        diffuse: [0.; 3],
        emission: [12., 3., 0.2],
        ..Material::AIR
    };
    let f = hdr::Accumulator::default()
        .update(&s, Quality::for_mode(Mode::Path))
        .unwrap();
    assert_eq!(f.emission[5], [12., 3., 0.2]);
    assert_eq!(f.light_field().rgba[5][0], 1.);
    s.materials[5].emission = [24., 6., 0.4];
    let b = hdr::Accumulator::default()
        .update(&s, Quality::for_mode(Mode::Path))
        .unwrap();
    assert_eq!(b.emission[5][0], 2. * f.emission[5][0]);
}
#[test]
fn sparse_opaque_particles_keep_their_gaps_and_prisms_have_front_faces() {
    let mut s = scene(8, 8, 1.);
    for y in [2, 4, 6] {
        let i = y * 8 + 4;
        s.input.optics[i] = Optical::OPAQUE;
        s.materials[i] = Material::STONE;
    }
    assert_eq!(
        hdr::visibility(&s, [1.5, 2.5, 0.], [6.5, 2.5, 0.], 64),
        [0.; 3]
    );
    assert_eq!(
        hdr::visibility(&s, [1.5, 3.5, 0.], [6.5, 3.5, 0.], 64),
        [1.; 3]
    );
    let h = hdr::intersect(&s, [4.5, 2.5, 12.], [0., 0., -1.], 16., 64).unwrap();
    assert_eq!(h.index, 20);
    assert_eq!(h.normal, [0., 0., 1.]);
    assert!((h.distance - 4.).abs() < 0.001);
}
#[test]
fn water_absorption_uses_distance_including_starting_cell_and_slab_exit() {
    for size in [1., 0.5] {
        let mut s = scene((8. / size) as u32, (4. / size) as u32, size);
        for m in &mut s.materials {
            *m = Material {
                absorption: [0.2, 0.1, 0.05],
                ..Material::WATER
            };
        }
        let t = hdr::visibility(&s, [1.25, 1.25, 0.], [5.25, 1.25, 0.], 128);
        for k in 0..3 {
            let expected = (-[0.2_f32, 0.1, 0.05][k] * 4.).exp();
            assert!((t[k] - expected).abs() < 0.001, "size {size}: {t:?}");
        }
        let t = hdr::visibility(&s, [1.25, 1.25, 7.], [1.25, 1.25, 10.], 128);
        assert!((t[0] - (-0.2_f32).exp()).abs() < 0.001, "slab exit {t:?}");
    }
}
#[test]
fn water_internal_cells_have_no_interfaces_and_fresnel_increases_at_grazing_angles() {
    let mut s = scene(8, 4, 1.);
    for m in &mut s.materials {
        *m = Material::WATER;
    }
    let hit = hdr::intersect(&s, [1.5, 1.5, 0.], [1., 0., 0.], 10., 64).unwrap();
    assert!((hit.distance - 6.5).abs() < 0.001);
    assert!(Material::WATER.fresnel(0.1) > Material::WATER.fresnel(1.));
    assert!((Material::WATER.fresnel(1.) - 0.02037).abs() < 0.0001);
}
#[test]
fn invalidation_covers_changes_without_discarding_distant_history() {
    let mut s = scene(64, 4, 1.);
    s.input.settings.max_distance = 4.;
    let old = s.clone();
    s.input.optics[1] = Optical::OPAQUE;
    let mask = hdr::invalidation(Some(&old), &s);
    assert_eq!(mask[1], 1);
    assert_eq!(mask[6], 1);
    assert_eq!(mask[40], 0);
    let q = Quality {
        samples_per_update: 1,
        max_samples: 4,
        ..Quality::for_mode(Mode::Path)
    };
    let mut acc = hdr::Accumulator::default();
    acc.update(&old, q).unwrap();
    acc.update(&old, q).unwrap();
    acc.update(&s, q).unwrap();
    assert_eq!(acc.counts()[1], 1);
    assert_eq!(acc.counts()[40], 3);
    s.input.settings.sky = [0.5; 3];
    assert!(hdr::invalidation(Some(&old), &s).iter().all(|v| *v == 1));
}
#[test]
fn invalid_materials_and_budgets_are_rejected() {
    let mut s = scene(4, 4, 1.);
    s.materials[0].ior = f32::NAN;
    assert!(s.validate().is_err());
    s.materials[0] = Material::STONE;
    s.surface_quality.transmission_bounces = 100;
    assert!(s.validate().is_err());
}

#[test]
fn material_emission_drives_lighting_and_static_water_keeps_accumulating() {
    let mut s = scene(8, 8, 1.);
    s.materials[18] = Material {
        emission: [12., 2., 0.1],
        dynamic: true,
        ..Material::AIR
    };
    assert_eq!(s.emitters(), vec![18]);
    assert_eq!(s.radiance(18), [12., 2., 0.1]);
    s.materials[50] = Material::WATER;
    let q = Quality {
        samples_per_update: 2,
        max_samples: 13,
        ..Quality::for_mode(Mode::Path)
    };
    let mut acc = hdr::Accumulator::default();
    let mut lit = 0_f32;
    for _ in 0..8 {
        let field = acc.update(&s, q).unwrap();
        lit = field.diffuse.iter().flatten().copied().fold(0., f32::max);
    }
    assert!(lit > 0.);
    assert_eq!(acc.counts()[50], 13);
    s.materials[18] = Material::AIR;
    let dark = acc.update(&s, q).unwrap();
    assert!(
        dark.diffuse
            .iter()
            .chain(&dark.specular)
            .chain(&dark.emission)
            .flatten()
            .all(|v| *v == 0.)
    );
}

#[test]
fn emissive_gas_does_not_turn_into_an_opaque_shadow_after_many_interfaces() {
    let mut s = scene(16, 4, 1.);
    for x in (1..14).step_by(2) {
        s.materials[16 + x] = Material {
            emission: [8., 1., 0.1],
            ..Material::AIR
        };
    }
    assert_eq!(
        hdr::visibility(&s, [0.5, 1.5, 0.], [15.5, 1.5, 0.], 64),
        [1.; 3]
    );
    assert_eq!(
        hdr::visibility(&s, [0.5, 1.5, 0.], [15.5, 1.5, 0.], 2),
        [0.; 3]
    );
    s.input.optics[24] = Optical::OPAQUE;
    s.materials[24] = Material::STONE;
    assert_eq!(
        hdr::visibility(&s, [-1., 1.5, 0.], [15.5, 1.5, 0.], 64),
        [0.; 3]
    );
}

#[test]
fn a_zero_range_does_not_sample_the_environment() {
    let mut s = scene(4, 4, 1.);
    s.input.settings.max_distance = 0.;
    s.input.settings.sky = [1.; 3];
    let f = hdr::Accumulator::default()
        .update(&s, Quality::for_mode(Mode::Path))
        .unwrap();
    assert!(
        f.diffuse
            .iter()
            .chain(&f.specular)
            .flatten()
            .all(|v| *v == 0.)
    );
}

#[test]
fn sharp_water_reflection_is_stable_without_temporal_accumulation() {
    let mut s = scene(16, 12, 1.);
    for y in 8..11 {
        for x in 2..14 {
            s.materials[y * 16 + x] = Material {
                normal: if y == 8 {
                    [0., -0.98, 0.19899748]
                } else {
                    [0., 0., 1.]
                },
                specular: if y == 8 { 0.02 } else { 0. },
                ..Material::WATER
            };
        }
    }
    for x in 0..16 {
        s.materials[11 * 16 + x] = Material::SAND;
        s.input.optics[11 * 16 + x] = Optical::OPAQUE;
    }
    for y in 3..6 {
        for x in 6..10 {
            s.materials[y * 16 + x] = Material {
                emission: [10., 4., 0.3],
                ..Material::AIR
            };
        }
    }
    let q = Quality::for_mode(Mode::Path);
    let first = hdr::sample(&s, q, 8 * 16 + 8, 0);
    assert!(
        first.specular.iter().any(|v| *v > 0.001),
        "test must contain a visible reflection or transmission"
    );
    for sample in 1..32 {
        let c = hdr::sample(&s, q, 8 * 16 + 8, sample);
        assert_eq!(
            c.specular, first.specular,
            "reflection flickered at sample {sample}"
        );
        assert_eq!(c.direct, first.direct);
    }
}
#[test]
fn compact_bright_lights_receive_probability_proportional_to_their_power() {
    let mut s = scene(16, 4, 1.);
    for i in 0..16 {
        s.materials[i] = Material {
            emission: [0.01; 3],
            ..Material::AIR
        };
    }
    s.input.sources[32] = [14.; 3];
    let distribution = hdr::emitter_distribution(&s, &s.emitters());
    let bright = distribution.iter().find(|v| v.0 == 32).unwrap();
    assert!(
        bright.2 > 0.99,
        "bright compact light probability {}",
        bright.2
    );
    assert_eq!(distribution.last().unwrap().1, 1.);
    assert!((distribution.iter().map(|v| v.2).sum::<f32>() - 1.).abs() < 1e-5);
    assert!(distribution.windows(2).all(|p| p[0].1 <= p[1].1));
}

#[test]
fn rare_indirect_samples_are_bounded_without_clamping_direct_or_emissive_hdr() {
    let mut s = scene(16, 8, 1.);
    s.input.settings.max_distance = 32.;
    for x in 0..16 {
        s.input.optics[6 * 16 + x] = Optical::OPAQUE;
        s.materials[6 * 16 + x] = Material::SAND;
    }
    s.input.sources[4 * 16 + 8] = [14., 7., 0.5];
    let q = Quality::for_mode(Mode::Path);
    let mut exercised = false;
    for seed in 0..128 {
        s.surface_quality.indirect_clamp = 0.;
        let raw = hdr::sample(&s, q, 3 * 16 + 3, seed);
        s.surface_quality.indirect_clamp = 0.001;
        let limited = hdr::sample(&s, q, 3 * 16 + 3, seed);
        let direct = raw.direct.into_iter().fold(0_f32, f32::max);
        let ceiling = direct.max(0.01) * 0.001;
        assert!(limited.indirect.iter().all(|v| *v <= ceiling + 1e-6));
        assert_eq!(limited.direct, raw.direct);
        assert_eq!(limited.specular, raw.specular);
        exercised |= raw.indirect.iter().any(|v| *v > ceiling);
    }
    assert!(exercised, "fixture must contain an indirect outlier");
}
