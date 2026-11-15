use crate::text_norm::normalize_math_text;

pub const MINHASH_LEN: usize = 128;

/// Computes a 128-hash MinHash signature over normalized 5-word shingles.
/// Returns `None` when the text contains too few tokens (< 20 words) to avoid spurious collisions.
pub fn compute_minhash(text: &str) -> Option<[u64; MINHASH_LEN]> {
    let norm = normalize_math_text(text);
    let words: Vec<&str> = norm
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    if words.len() < 20 {
        return None;
    }

    let mut sig = [u64::MAX; MINHASH_LEN];
    let window_size = words.len().min(5);
    for win in words.windows(window_size) {
        let base_h = fnv1a_shingle(win);
        for (i, slot) in sig.iter_mut().enumerate() {
            let h = mix_permutation(base_h, i as u64);
            if h < *slot {
                *slot = h;
            }
        }
    }
    Some(sig)
}

pub fn pack_minhash(sig: &[u64; MINHASH_LEN]) -> Vec<u8> {
    let mut out = Vec::with_capacity(MINHASH_LEN * 8);
    for v in sig {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

pub fn unpack_minhash(blob: &[u8]) -> Option<[u64; MINHASH_LEN]> {
    if blob.len() != MINHASH_LEN * 8 {
        return None;
    }
    let mut out = [0u64; MINHASH_LEN];
    let (chunks, _) = blob.as_chunks::<8>();
    for (i, chunk) in chunks.iter().enumerate() {
        out[i] = u64::from_le_bytes(*chunk);
    }
    Some(out)
}

pub fn jaccard_similarity(a: &[u64; MINHASH_LEN], b: &[u64; MINHASH_LEN]) -> f32 {
    let equal = a.iter().zip(b.iter()).filter(|(x, y)| x == y).count();
    equal as f32 / MINHASH_LEN as f32
}

fn fnv1a_shingle(words: &[&str]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for (idx, w) in words.iter().enumerate() {
        if idx > 0 {
            h ^= 0xff;
            h = h.wrapping_mul(0x100000001b3);
        }
        for &b in w.as_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
    }
    h
}

#[inline]
fn mix_permutation(x: u64, idx: u64) -> u64 {
    let k1 = idx.wrapping_mul(0x9e3779b97f4a7c15).wrapping_add(0xbf58476d1ce4e5b9);
    let k2 = idx.wrapping_mul(0x94d049bb133111eb).wrapping_add(0x27d4eb2f165667c5);
    let mut z = x.wrapping_mul(k1 | 1).wrapping_add(k2);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_and_near_identical_texts_score_high() {
        let base = "Let X be a compact Kahler manifold with positive holomorphic sectional curvature. \
                    We prove by a Bochner-type formula and the Schwarz lemma that every rational curve \
                    on X satisfies the stability condition under Ricci flow perturbation.";
        let edited = "Let X be a compact Kähler manifold with positive holomorphic sectional curvature. \
                      We prove by a Bochner-type formula and the Schwarz lemma that every rational curve \
                      on X satisfies the stability condition under Ricci flow.";
        let unrelated = "In modular representation theory of finite groups of Lie type, Lusztig characters \
                         govern the decomposition numbers of blocks of defect zero and cyclic defect.";

        let s1 = compute_minhash(base).expect("sig1");
        let s2 = compute_minhash(edited).expect("sig2");
        let s3 = compute_minhash(unrelated).expect("sig3");

        let sim_close = jaccard_similarity(&s1, &s2);
        let sim_far = jaccard_similarity(&s1, &s3);
        assert!(sim_close > 0.70, "expected close similarity, got {sim_close}");
        assert!(sim_far < 0.25, "expected low similarity, got {sim_far}");

        let round = unpack_minhash(&pack_minhash(&s1)).expect("roundtrip");
        assert_eq!(s1, round);
    }
}
