"""Check optional feature boundaries using Cargo's resolved dependency graph."""
import json
import subprocess

data=json.loads(subprocess.check_output(['cargo','metadata','--locked','--format-version','1','--no-deps'],text=True))
packages={p['name']:p for p in data['packages']}
def tree(package):
    return subprocess.check_output(['cargo','tree','--locked','-p',package,'--no-default-features','--prefix','none','--format','{p}'],text=True)
for package in ['charon_tilemap','charon_render','charon_assets','charon_tilemap_render']:
    graph=tree(package)
    assert 'charon_physics ' not in graph, f'{package} unexpectedly requires physics'
    if package in ['charon_tilemap','charon_render']:
        assert 'wgpu ' not in graph and 'winit ' not in graph, f'{package} requires window/GPU without opt-in'
assert 'charon-core ' not in tree('charon_tilemap'), 'tile data must work without Core'
assert 'charon_physics ' not in tree('charon-m3-sample'), 'tile-only sample must compile without physics'
assert 'charon_bridge ' not in tree('charon-m3-sample'), 'bridge must remain optional'
print('M3: Tilemap, generic Rendering, Assets and tile-only sample dependency boundaries OK')
