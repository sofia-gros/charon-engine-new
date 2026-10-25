"""Resolve facade features separately; workspace feature unification cannot hide coupling."""
import subprocess


def graph(package, *features):
    return subprocess.check_output(
        ["cargo", "tree", "--locked", "-p", package, "--no-default-features", "--edges", "normal", "--prefix", "none", "--format", "{p}", *features],
        text=True,
    )


base = graph("charon_engine")
assert {line.split()[0] for line in base.splitlines()} == {"charon_engine", "charon_core"}
for feature in ["input", "camera", "scenes", "assets", "interaction", "lighting", "tilemap", "render"]:
    resolved = graph("charon_engine", "--features", feature)
    for forbidden in ["charon_physics ", "charon_sand ", "charon_bridge ", "wgpu ", "winit "]:
        assert forbidden not in resolved, (feature, forbidden)
for package in ["charon_physics", "charon_tilemap", "charon_lighting", "charon_camera", "charon_input", "charon_scene"]:
    resolved = graph(package)
    assert "charon_engine " not in resolved and "charon_core " not in resolved, package
    assert "wgpu " not in resolved and "winit " not in resolved, package
assert "charon_physics " not in graph("charon_render")
assert "wgpu " not in graph("charon_render")
for feature in ["physics", "sand", "bridge"]:
    assert "wgpu " not in graph("charon_engine", "--features", feature)
assert "wgpu " in graph("charon_engine", "--features", "gpu")
assert "charon_physics " not in graph("charon_engine", "--features", "gpu")
for features in [(), ("--features", "lighting")]:
    assert "charon_physics " not in graph("charon-m6-sample", *features)
print("M6: core-only facade, independent APIs, feature-selected backends, Sand-free sample OK")
