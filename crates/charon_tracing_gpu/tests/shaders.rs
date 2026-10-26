use charon_tracing_gpu::*;
#[test]
fn portable_and_rtx_wgsl_parse_and_validate_without_an_rtx_device() {
    for mode in [Mode::Path, Mode::LitePath, Mode::Rtx] {
        for text in [shader(mode), hdr_shader(mode)] {
            let module = naga::front::wgsl::parse_str(&text)
                .unwrap_or_else(|e| panic!("{}", e.emit_to_string(&text)));
            naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::all(),
            )
            .validate(&module)
            .unwrap();
        }
    }
}

#[test]
fn hdr_composite_and_postprocess_validate() {
    for text in [
        RTX_GEOMETRY_PROBE,
        include_str!("../../charon_lighting_render/src/hdr_composite.wgsl"),
        include_str!("../../charon_render/src/hdr.wgsl"),
    ] {
        let module = naga::front::wgsl::parse_str(text)
            .unwrap_or_else(|e| panic!("{}", e.emit_to_string(text)));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap();
    }
}
