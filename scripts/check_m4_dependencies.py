"""Resolve the independent optical core, adapters and optional sample modes."""
import subprocess

def tree(package, *features):
    return subprocess.check_output(["cargo", "tree", "--locked", "-p", package, "--no-default-features", "--prefix", "none", "--format", "{p}", *features], text=True)
core = tree("charon_lighting")
assert len(core.splitlines()) == 1, "Optical reference core must be std-only"
for package in ["charon_lighting_gpu", "charon_lighting_render", "charon_tilemap_lighting"]:
    graph = tree(package)
    assert "charon_physics " not in graph and "charon_bridge " not in graph, package
assert "charon_render " not in tree("charon_lighting_gpu")
assert "wgpu " not in tree("charon_sand_lighting")
assert "charon_lighting " not in tree("charon_render")
plain = tree("charon-m4-sample")
assert "charon_lighting " not in plain and "charon_physics " not in plain
lit_tiles = tree("charon-m4-sample", "--features", "lighting")
assert "charon_lighting " in lit_tiles and "charon_physics " not in lit_tiles
print("M4: std-only optics, separate GPU computation/composition, optional Sand and Lighting OK")
