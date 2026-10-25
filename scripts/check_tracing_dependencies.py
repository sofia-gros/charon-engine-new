"""Tracing consumes common optics, with no Tilemap/Sand/renderer dependency."""
import subprocess

def graph(package, *features):
    return subprocess.check_output(['cargo','tree','--locked','-p',package,'--no-default-features','--edges','normal','--prefix','none','--format','{p}',*features],text=True)
for package in ['charon_tracing','charon_tracing_gpu']:
    value=graph(package)
    for forbidden in ['charon_physics ','charon_tilemap ','charon_render ','charon_engine ','winit ']:
        assert forbidden not in value,(package,forbidden)
assert set(x.split()[0] for x in graph('charon_tracing').splitlines())=={'charon_tracing','charon_lighting'}
assert 'wgpu ' not in graph('charon_engine','--features','tracing')
assert 'charon_physics ' not in graph('charon-rt-sample')
print('Tracing: std optical/reference layer; independent GPU/plugin; optional RTX and Falling Sand OK')
from pathlib import Path
for package in ['charon_tracing', 'charon_tracing_gpu']:
    manifest = Path('crates', package, 'Cargo.toml').read_text(encoding='utf-8')
    assert 'serde' not in manifest, 'configuration serialization must stay in the demo'
    for source in Path('crates', package, 'src').glob('*.rs'):
        text = source.read_text(encoding='utf-8')
        assert 'std::fs' not in text and 'serde_json' not in text
print('Tracing plugin: capability/switch APIs only; settings UI and file I/O are demo-owned')
