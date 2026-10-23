//! Aggregation multigrid coefficients. All V-cycle vectors and operations stay
//! on GPU. CPU only builds this changing pressure operator and a bounded coarse
//! inverse; the original f64 residual remains the convergence authority.
use super::{preconditioner, Error};
use std::collections::BTreeMap;

#[derive(Clone)]
struct Node {
    edges: Vec<(usize, f64)>,
    boundary: f64,
    diagonal: f64,
}
pub(super) const COARSE_SWEEPS: usize = 16;
pub(super) struct Hierarchy {
    pub nodes: Vec<u8>,
    pub data: Vec<u8>,
    pub params: Vec<u8>,
    pub counts: Vec<usize>,
    pub direct: bool,
}
type Row = ([usize; 4], [f64; 4], f64);

fn aggregate(nodes: &[Node]) -> (Vec<usize>, usize) {
    let mut parent = vec![usize::MAX; nodes.len()];
    let mut count = 0;
    for seed in 0..nodes.len() {
        if parent[seed] != usize::MAX {
            continue;
        }
        let mut group = vec![seed];
        parent[seed] = count;
        let mut cursor = 0;
        while cursor < group.len() && group.len() < 8 {
            let i = group[cursor];
            cursor += 1;
            for &(j, _) in &nodes[i].edges {
                if parent[j] == usize::MAX && group.len() < 8 {
                    parent[j] = count;
                    group.push(j);
                }
            }
        }
        count += 1;
    }
    (parent, count)
}

fn coarse(nodes: &[Node], parent: &[usize], count: usize) -> Vec<Node> {
    let mut result = vec![
        Node {
            edges: Vec::new(),
            boundary: 0.,
            diagonal: 0.
        };
        count
    ];
    let mut edges = BTreeMap::new();
    for (i, node) in nodes.iter().enumerate() {
        let a = parent[i];
        result[a].boundary += node.boundary;
        for &(j, weight) in &node.edges {
            let b = parent[j];
            if j > i && a != b {
                *edges.entry((a.min(b), a.max(b))).or_insert(0.) += weight;
            }
        }
    }
    for ((a, b), weight) in edges {
        result[a].edges.push((b, weight));
        result[b].edges.push((a, weight));
    }
    for node in &mut result {
        node.edges.sort_unstable_by_key(|&(j, _)| j);
        node.diagonal = node.boundary + node.edges.iter().map(|e| e.1).sum::<f64>();
    }
    result
}

fn coarse_inverse(nodes: &[Node]) -> Result<Vec<f64>, Error> {
    let n = nodes.len();
    let mut closed = Vec::new();
    let mut seen = vec![false; n];
    for start in 0..n {
        if seen[start] {
            continue;
        }
        let mut component = vec![start];
        seen[start] = true;
        let mut cursor = 0;
        let mut open = false;
        while cursor < component.len() {
            let i = component[cursor];
            cursor += 1;
            open |= nodes[i].boundary > 1e-12 * nodes[i].diagonal.max(1e-20);
            for &(j, _) in &nodes[i].edges {
                if !seen[j] {
                    seen[j] = true;
                    component.push(j);
                }
            }
        }
        if !open {
            closed.push(component);
        }
    }
    let mut pinned = vec![false; n];
    for c in &closed {
        pinned[c[0]] = true;
    }
    let mut matrix = vec![0.; n * n];
    for (i, node) in nodes.iter().enumerate() {
        matrix[i * n + i] = if pinned[i] {
            1.
        } else {
            node.diagonal.max(1e-20)
        };
        if pinned[i] {
            continue;
        }
        for &(j, a) in &node.edges {
            if !pinned[j] {
                matrix[i * n + j] = -a;
            }
        }
    }
    let mut inverse = preconditioner::inverse(matrix, n)?;
    // C (pinned inverse) C is the symmetric pseudoinverse on each closed
    // component. It neither pins the fine operator nor couples distinct lakes.
    for c in closed {
        let means: Vec<_> = c
            .iter()
            .map(|&i| c.iter().map(|&j| inverse[i * n + j]).sum::<f64>() / c.len() as f64)
            .collect();
        let mean = means.iter().sum::<f64>() / c.len() as f64;
        for (a, &i) in c.iter().enumerate() {
            for (b, &j) in c.iter().enumerate() {
                inverse[i * n + j] -= means[a] + means[b] - mean;
            }
        }
    }
    Ok(inverse)
}

impl Hierarchy {
    pub fn build(rows: &[Row]) -> Result<Self, Error> {
        let fine: Vec<_> = rows
            .iter()
            .map(|(adj, a, diagonal)| {
                let edges: Vec<_> = (0..4)
                    .filter(|&k| adj[k] != usize::MAX && a[k] > 0.)
                    .map(|k| (adj[k], a[k]))
                    .collect();
                // Tiny subtraction roundoff in the preconditioner must not invent
                // a negative boundary term. The original operator is untouched.
                let boundary = (diagonal - edges.iter().map(|e| e.1).sum::<f64>()).max(0.);
                Node {
                    edges,
                    boundary,
                    diagonal: *diagonal,
                }
            })
            .collect();
        let mut graphs = vec![fine];
        let mut parents = Vec::new();
        while graphs.last().unwrap().len() > 128 && graphs.len() < 8 {
            let nodes = graphs.last().unwrap();
            let (parent, count) = aggregate(nodes);
            // Thousands of separate drops must not produce a dense N*N
            // inverse. Stagnation uses bounded symmetric Jacobi sweeps instead.
            if count * 4 >= nodes.len() * 3 {
                break;
            }
            graphs.push(coarse(nodes, &parent, count));
            parents.push(parent);
        }
        let counts: Vec<_> = graphs.iter().map(Vec::len).collect();
        let mut offsets = vec![0; counts.len()];
        for i in 1..counts.len() {
            offsets[i] = offsets[i - 1] + counts[i - 1];
        }
        let mut words = Vec::<u32>::new();
        let mut encoded = Vec::<u32>::new();
        for (level, graph) in graphs.iter().enumerate() {
            let mut children = vec![Vec::new(); graph.len()];
            if level > 0 {
                for (i, &p) in parents[level - 1].iter().enumerate() {
                    children[p].push((offsets[level - 1] + i) as u32);
                }
            }
            for (i, node) in graph.iter().enumerate() {
                let first = words.len();
                for &(j, weight) in &node.edges {
                    words.extend([(offsets[level] + j) as u32, (weight as f32).to_bits()]);
                }
                let child_first = words.len();
                words.extend(&children[i]);
                let parent = if level + 1 < graphs.len() {
                    (offsets[level + 1] + parents[level][i]) as u32
                } else {
                    u32::MAX
                };
                encoded.extend([
                    first as u32,
                    node.edges.len() as u32,
                    parent,
                    child_first as u32,
                    children[i].len() as u32,
                    0,
                    (node.diagonal.max(1e-20) as f32).to_bits(),
                    (node.boundary as f32).to_bits(),
                ]);
            }
        }
        let inverse_offset = words.len();
        let last = graphs.last().unwrap();
        let direct = last.len() <= 128;
        if direct {
            for value in coarse_inverse(last)? {
                let value = value as f32;
                if !value.is_finite() {
                    return Err(Error::Pressure);
                }
                words.push(value.to_bits());
            }
        }
        let mut params = vec![0; counts.len() * 256];
        for i in 0..counts.len() {
            let values = [
                offsets[i],
                counts[i],
                *offsets.get(i + 1).unwrap_or(&0),
                *counts.get(i + 1).unwrap_or(&0),
                inverse_offset,
                usize::from(direct),
                0,
                0,
            ];
            for (j, v) in values.into_iter().enumerate() {
                params[i * 256 + j * 4..i * 256 + j * 4 + 4]
                    .copy_from_slice(&(v as u32).to_le_bytes());
            }
        }
        // A runtime-sized WGSL array requires at least one bound element.
        if words.is_empty() {
            words.push(0);
        }
        Ok(Self {
            nodes: encoded.into_iter().flat_map(u32::to_le_bytes).collect(),
            data: words.into_iter().flat_map(u32::to_le_bytes).collect(),
            params,
            counts,
            direct,
        })
    }
    pub fn dispatches(&self) -> u32 {
        // Zero-start first sweep is fused into RHS/restriction. Prolongation
        // includes the first post-sweep; the final fine sweep includes PCG dots.
        let last = self.counts.len() - 1;
        (4 * last + if self.direct { 1 } else { COARSE_SWEEPS - 1 } + 2
            - usize::from(last > 0 || !self.direct)) as u32
    }
}
