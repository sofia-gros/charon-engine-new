//! Two-level additive Schwarz: exact local block inverses plus a Galerkin
//! coarse inverse. CPU prepares coefficients once; GPU applies both levels on
//! every Krylov iteration. No change to the original pressure operator/tolerance.
use crate::Error;
type Row = ([usize; 4], [f64; 4], f64);

pub(super) fn inverse(mut a: Vec<f64>, n: usize) -> Result<Vec<f64>, Error> {
    // Cholesky in f64, including the gauge-pinned closed components.
    for i in 0..n {
        for j in 0..=i {
            let v = a[i * n + j] - (0..j).map(|k| a[i * n + k] * a[j * n + k]).sum::<f64>();
            a[i * n + j] = if i == j {
                if !v.is_finite() || v <= 0. {
                    return Err(Error::Pressure);
                }
                v.sqrt()
            } else {
                v / a[j * n + j]
            };
        }
    }
    let mut result = vec![0.; n * n];
    let mut x = vec![0.; n];
    for column in 0..n {
        for i in 0..n {
            x[i] = (f64::from(i == column) - (0..i).map(|j| a[i * n + j] * x[j]).sum::<f64>())
                / a[i * n + i];
        }
        for i in (0..n).rev() {
            x[i] = (x[i] - (i + 1..n).map(|j| a[j * n + i] * x[j]).sum::<f64>()) / a[i * n + i];
            result[i * n + column] = x[i];
        }
    }
    // Explicit symmetry avoids small independently-rounded inverse entries
    // violating the SPD preconditioner contract used by conjugate gradients.
    for i in 0..n {
        for j in 0..i {
            let v = (result[i * n + j] + result[j * n + i]) * 0.5;
            result[i * n + j] = v;
            result[j * n + i] = v;
        }
    }
    Ok(result)
}

pub(super) fn build(rows: &[Row], anchors: &[bool]) -> Result<Vec<u8>, Error> {
    let n = rows.len();
    // Grow local blocks with the problem, keeping the coarse matrix small.
    let block_size = n.div_ceil(128).next_power_of_two().max(8);
    // Connected aggregates, rather than strips in compact row-number order.
    // Disconnected liquid components never share an aggregate.
    let mut location = vec![usize::MAX; n];
    let mut groups = Vec::new();
    for seed in 0..n {
        if location[seed] != usize::MAX {
            continue;
        }
        let block = groups.len();
        let mut group = vec![seed];
        location[seed] = block * block_size;
        let mut cursor = 0;
        while cursor < group.len() && group.len() < block_size {
            let i = group[cursor];
            cursor += 1;
            for &j in &rows[i].0 {
                if j != usize::MAX && location[j] == usize::MAX && group.len() < block_size {
                    location[j] = block * block_size + group.len();
                    group.push(j);
                }
            }
        }
        groups.push(group);
    }
    let blocks = groups.len();
    // Bound dense coarse preparation even for thousands of isolated droplets.
    if blocks > 256 {
        return Ok(vec![0; 20]);
    }
    let indices_offset = n * (block_size + 1);
    let coarse_offset = indices_offset + blocks * block_size;
    let rhs_offset = coarse_offset + blocks * blocks;
    let mut values = vec![0.; rhs_offset + 2 * blocks];
    values[indices_offset..coarse_offset].fill(-1.);
    for (i, &slot) in location.iter().enumerate() {
        values[n * block_size + i] = slot as f64;
    }
    let mut coarse = vec![0.; blocks * blocks];
    for (block, group) in groups.iter().enumerate() {
        let size = group.len();
        let mut local = vec![0.; size * size];
        for (slot, &i) in group.iter().enumerate() {
            values[indices_offset + block * block_size + slot] = i as f64;
            let (adj, a, diagonal) = rows[i];
            let d = if anchors[i] { 1. } else { diagonal };
            local[slot * size + slot] = d;
            coarse[block * blocks + block] += d;
            if anchors[i] {
                continue;
            }
            for k in 0..4 {
                let j = adj[k];
                if j == usize::MAX || anchors[j] {
                    continue;
                }
                let other = location[j] / block_size;
                coarse[block * blocks + other] -= a[k];
                if other == block {
                    local[slot * size + location[j] % block_size] -= a[k];
                }
            }
        }
        let local = inverse(local, size)?;
        for i in 0..size {
            for j in 0..size {
                values[group[i] * block_size + j] = local[i * size + j];
            }
        }
    }
    let coarse = inverse(coarse, blocks)?;
    values[coarse_offset..rhs_offset].copy_from_slice(&coarse);
    let mut bytes = Vec::with_capacity(16 + values.len() * 4);
    for v in [blocks, block_size, coarse_offset, rhs_offset] {
        bytes.extend_from_slice(&(v as u32).to_le_bytes());
    }
    for v in values {
        let v = v as f32;
        if !v.is_finite() {
            return Err(Error::Pressure);
        }
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    Ok(bytes)
}
