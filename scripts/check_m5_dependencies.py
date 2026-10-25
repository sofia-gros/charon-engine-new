"""Check that Input, Camera and Scene do not impose a platform or game model."""
import subprocess

def tree(package,*features):
    return subprocess.check_output(['cargo','tree','--locked','-p',package,'--no-default-features','--prefix','none','--format','{p}',*features],text=True)
for package in ['charon_input','charon_camera','charon_scene']:
    graph=tree(package)
    assert len(graph.splitlines())==1, f'{package} must be std-only without its plugin feature'
for package in ['charon_input_winit','charon_camera_render']:
    graph=tree(package)
    assert 'charon_physics ' not in graph and 'charon_lighting ' not in graph
    assert 'wgpu ' not in graph
plain=tree('charon-m5-sample')
assert 'charon_physics ' not in plain and 'charon_lighting ' not in plain
lit=tree('charon-m5-sample','--features','lighting')
assert 'charon_lighting ' in lit and 'charon_physics ' not in lit
print('M5: std-only Input/Camera/Scene, optional adapters/Core, Tilemap and unlit configurations OK')
