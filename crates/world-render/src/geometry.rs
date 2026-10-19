//! Geometry shared by every frame, built once per renderer: the landscape grid's indices and the
//! unit sphere. Neither depends on the world; positions come from the cells and the body state.

use world_model::spatial::CHUNK_SIZE;

/// Vertices along each side of a chunk's layer grid: one per cell plus the neighbour's first.
pub const VERTS_ACROSS: u32 = CHUNK_SIZE as u32 + 1;
pub const TERRAIN_INDEX_COUNT: u32 = (CHUNK_SIZE * CHUNK_SIZE * 6) as u32;

/// Two triangles per cell quad, split along the diagonal from `(x + 1, y)` to `(x, y + 1)`: the
/// split the body kernels' surface uses (gpu-world `body/geometry.wgsl`), so a sphere rests on
/// the triangles it is drawn on.
pub fn terrain_indices() -> Vec<u16> {
    let mut indices = Vec::with_capacity(TERRAIN_INDEX_COUNT as usize);
    let across = VERTS_ACROSS as u16;
    for y in 0..CHUNK_SIZE as u16 {
        for x in 0..CHUNK_SIZE as u16 {
            let i = y * across + x;
            indices.extend_from_slice(&[i, i + 1, i + across, i + 1, i + across + 1, i + across]);
        }
    }
    indices
}

/// A unit icosphere subdivided `levels` times: positions (equal to normals) and indices.
pub fn icosphere(levels: u32) -> (Vec<[f32; 3]>, Vec<u16>) {
    let t = (1.0 + 5.0f32.sqrt()) / 2.0;
    let mut vertices: Vec<[f32; 3]> = [
        [-1.0, t, 0.0],
        [1.0, t, 0.0],
        [-1.0, -t, 0.0],
        [1.0, -t, 0.0],
        [0.0, -1.0, t],
        [0.0, 1.0, t],
        [0.0, -1.0, -t],
        [0.0, 1.0, -t],
        [t, 0.0, -1.0],
        [t, 0.0, 1.0],
        [-t, 0.0, -1.0],
        [-t, 0.0, 1.0],
    ]
    .into_iter()
    .map(normalize)
    .collect();
    let mut faces: Vec<[u16; 3]> = vec![
        [0, 11, 5],
        [0, 5, 1],
        [0, 1, 7],
        [0, 7, 10],
        [0, 10, 11],
        [1, 5, 9],
        [5, 11, 4],
        [11, 10, 2],
        [10, 7, 6],
        [7, 1, 8],
        [3, 9, 4],
        [3, 4, 2],
        [3, 2, 6],
        [3, 6, 8],
        [3, 8, 9],
        [4, 9, 5],
        [2, 4, 11],
        [6, 2, 10],
        [8, 6, 7],
        [9, 8, 1],
    ];
    for _ in 0..levels {
        let mut midpoints = std::collections::HashMap::new();
        let mut midpoint = |a: u16, b: u16, vertices: &mut Vec<[f32; 3]>| -> u16 {
            let key = (a.min(b), a.max(b));
            *midpoints.entry(key).or_insert_with(|| {
                let (p, q) = (vertices[a as usize], vertices[b as usize]);
                vertices.push(normalize([
                    (p[0] + q[0]) * 0.5,
                    (p[1] + q[1]) * 0.5,
                    (p[2] + q[2]) * 0.5,
                ]));
                (vertices.len() - 1) as u16
            })
        };
        let mut next = Vec::with_capacity(faces.len() * 4);
        for [a, b, c] in faces {
            let ab = midpoint(a, b, &mut vertices);
            let bc = midpoint(b, c, &mut vertices);
            let ca = midpoint(c, a, &mut vertices);
            next.extend([[a, ab, ca], [b, bc, ab], [c, ca, bc], [ab, bc, ca]]);
        }
        faces = next;
    }
    (vertices, faces.into_iter().flatten().collect())
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let length = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    [v[0] / length, v[1] / length, v[2] / length]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_terrain_grid_covers_every_cell_quad_once() {
        let indices = terrain_indices();
        assert_eq!(indices.len() as u32, TERRAIN_INDEX_COUNT);
        assert_eq!(
            *indices.iter().max().expect("indices"),
            (VERTS_ACROSS * VERTS_ACROSS - 1) as u16
        );
        // The first quad's diagonal runs from (1, 0) to (0, 1).
        assert_eq!(&indices[..6], &[0, 1, 65, 1, 66, 65]);
    }

    #[test]
    fn the_icosphere_is_a_closed_unit_sphere() {
        let (vertices, indices) = icosphere(3);
        assert_eq!(vertices.len(), 642);
        assert_eq!(indices.len(), 1280 * 3);
        for v in &vertices {
            let length = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            assert!((length - 1.0).abs() < 1e-5);
        }
        // Counter-clockwise seen from outside: every face's normal points away from the centre,
        // so back-face culling keeps the outside.
        for face in indices.chunks(3) {
            let [a, b, c] = [0, 1, 2].map(|k| vertices[face[k] as usize]);
            let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let w = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            let n = [
                u[1] * w[2] - u[2] * w[1],
                u[2] * w[0] - u[0] * w[2],
                u[0] * w[1] - u[1] * w[0],
            ];
            assert!(n[0] * a[0] + n[1] * a[1] + n[2] * a[2] > 0.0);
        }
    }
}
