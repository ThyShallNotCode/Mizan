//! Small dense linear algebra in `f64` (no external crate): Gaussian elimination with partial
//! pivoting and the cyclic Jacobi eigen-solver for symmetric matrices. Loops run in a fixed order
//! and use no fused multiply-add, so results are reproducible (§8.11 geometry solves).

/// Solves `a · x = b` in place (`a` is `n × n` row-major; the solution replaces `b`).
/// Returns `None` if the matrix is singular to working precision.
pub fn solve(a: &mut [f64], b: &mut [f64], n: usize) -> Option<()> {
    debug_assert_eq!(a.len(), n * n);
    debug_assert_eq!(b.len(), n);
    let scale = a.iter().fold(0.0f64, |m, v| m.max(v.abs()));
    if scale == 0.0 || !scale.is_finite() {
        return None;
    }
    let eps = scale * 1e-13;
    for col in 0..n {
        let mut piv = col;
        let mut best = a[col * n + col].abs();
        for r in col + 1..n {
            let v = a[r * n + col].abs();
            if v > best {
                best = v;
                piv = r;
            }
        }
        if best <= eps {
            return None;
        }
        if piv != col {
            for k in 0..n {
                a.swap(col * n + k, piv * n + k);
            }
            b.swap(col, piv);
        }
        let d = a[col * n + col];
        for r in col + 1..n {
            let f = a[r * n + col] / d;
            if f == 0.0 {
                continue;
            }
            for k in col..n {
                a[r * n + k] -= f * a[col * n + k];
            }
            b[r] -= f * b[col];
        }
    }
    for col in (0..n).rev() {
        let mut s = b[col];
        for k in col + 1..n {
            s -= a[col * n + k] * b[k];
        }
        b[col] = s / a[col * n + col];
        if !b[col].is_finite() {
            return None;
        }
    }
    Some(())
}

/// Eigen-decomposition of a symmetric `n × n` matrix by cyclic Jacobi rotations.
/// Returns `(eigenvalues, eigenvectors)` with eigenvector `k` in column `k` of the row-major
/// matrix.
pub fn sym_eigen(a: &[f64], n: usize) -> (Vec<f64>, Vec<f64>) {
    let mut m = a.to_vec();
    let mut v = vec![0.0; n * n];
    for i in 0..n {
        v[i * n + i] = 1.0;
    }
    for _sweep in 0..100 {
        let mut off = 0.0;
        for p in 0..n {
            for q in p + 1..n {
                off += m[p * n + q] * m[p * n + q];
            }
        }
        if off < 1e-30 {
            break;
        }
        for p in 0..n {
            for q in p + 1..n {
                let apq = m[p * n + q];
                if apq.abs() < 1e-300 {
                    continue;
                }
                let app = m[p * n + p];
                let aqq = m[q * n + q];
                let theta = (aqq - app) / (2.0 * apq);
                let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
                let t = if theta == 0.0 { 1.0 } else { t };
                let c = 1.0 / (t * t + 1.0).sqrt();
                let s = t * c;
                for k in 0..n {
                    let mkp = m[k * n + p];
                    let mkq = m[k * n + q];
                    m[k * n + p] = c * mkp - s * mkq;
                    m[k * n + q] = s * mkp + c * mkq;
                }
                for k in 0..n {
                    let mpk = m[p * n + k];
                    let mqk = m[q * n + k];
                    m[p * n + k] = c * mpk - s * mqk;
                    m[q * n + k] = s * mpk + c * mqk;
                }
                for k in 0..n {
                    let vkp = v[k * n + p];
                    let vkq = v[k * n + q];
                    v[k * n + p] = c * vkp - s * vkq;
                    v[k * n + q] = s * vkp + c * vkq;
                }
            }
        }
    }
    ((0..n).map(|i| m[i * n + i]).collect(), v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solves_3x3() {
        let mut a = vec![2.0, 1.0, -1.0, -3.0, -1.0, 2.0, -2.0, 1.0, 2.0];
        let mut b = vec![8.0, -11.0, -3.0];
        solve(&mut a, &mut b, 3).unwrap();
        for (x, e) in b.iter().zip([2.0, 3.0, -1.0]) {
            assert!((x - e).abs() < 1e-12);
        }
    }

    #[test]
    fn singular_is_none() {
        let mut a = vec![1.0, 2.0, 2.0, 4.0];
        let mut b = vec![1.0, 2.0];
        assert!(solve(&mut a, &mut b, 2).is_none());
    }

    #[test]
    fn eigen_of_diagonalisable() {
        let a = [4.0, 1.0, 1.0, 3.0];
        let (vals, vecs) = sym_eigen(&a, 2);
        for k in 0..2 {
            let (x, y) = (vecs[k], vecs[2 + k]);
            let ax = 4.0 * x + 1.0 * y;
            let ay = 1.0 * x + 3.0 * y;
            assert!((ax - vals[k] * x).abs() < 1e-10 && (ay - vals[k] * y).abs() < 1e-10);
        }
    }
}
