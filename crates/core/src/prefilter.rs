//! Packed letter/digit masks for a Catalog. SIMD scan rejects names that
//! cannot fuzzy-match before nucleo sees them.
//!
//! Filenames are too short for per-name SIMD (setup beats the work). The
//! mask table is SoA `u64` per id — one sequential scan of the Catalog.

/// Bits 0–25: `a–z`. Bits 26–35: `0–9`.
#[inline]
pub(crate) fn mask_name(bytes: &[u8]) -> u64 {
    let mut m = 0u64;
    for &b in bytes {
        let l = b.to_ascii_lowercase();
        if l.is_ascii_lowercase() {
            m |= 1u64 << (l - b'a');
        } else if l.is_ascii_digit() {
            m |= 1u64 << (26 + (l - b'0'));
        }
    }
    m
}

#[inline]
pub(crate) fn needle_mask(parts: &[&str]) -> u64 {
    let mut m = 0u64;
    for p in parts {
        m |= mask_name(p.as_bytes());
    }
    m
}

/// Append `base + i` for every `i` where `(masks[i] & need) == need`.
pub(crate) fn scan_mask(masks: &[u64], need: u64, base: u32, out: &mut Vec<u32>) {
    if need == 0 {
        out.extend(base..base.saturating_add(masks.len() as u32));
        return;
    }
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx512f") {
            // SAFETY: feature bit checked above.
            unsafe {
                scan_avx512(masks, need, base, out);
            }
            return;
        }
        if is_x86_feature_detected!("avx2") {
            // SAFETY: feature bit checked above.
            unsafe {
                scan_avx2(masks, need, base, out);
            }
            return;
        }
    }
    scan_scalar(masks, need, base, out);
}

pub(crate) fn scan_scalar(masks: &[u64], need: u64, base: u32, out: &mut Vec<u32>) {
    for (i, &m) in masks.iter().enumerate() {
        if m & need == need {
            out.push(base.saturating_add(i as u32));
        }
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn scan_avx2(masks: &[u64], need: u64, base: u32, out: &mut Vec<u32>) {
    use core::arch::x86_64::*;

    let n = masks.len();
    let mut i = 0;
    let vneed = _mm256_set1_epi64x(need as i64);
    let ptr = masks.as_ptr();

    while i + 4 <= n {
        // SAFETY: `i + 4 <= n`, so 32 bytes from `ptr.add(i)` are in-bounds.
        // `loadu` accepts unaligned addresses (Vec<u64> is 8-aligned, not 32).
        let v = unsafe { _mm256_loadu_si256(ptr.add(i).cast()) };
        let vand = _mm256_and_si256(v, vneed);
        let eq = _mm256_cmpeq_epi64(vand, vneed);
        let bits = _mm256_movemask_pd(_mm256_castsi256_pd(eq)) as u8;
        if bits != 0 {
            for lane in 0..4 {
                if bits & (1 << lane) != 0 {
                    out.push(base.saturating_add((i + lane) as u32));
                }
            }
        }
        i += 4;
    }
    while i < n {
        // SAFETY: `i < n`.
        let m = unsafe { *ptr.add(i) };
        if m & need == need {
            out.push(base.saturating_add(i as u32));
        }
        i += 1;
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f")]
unsafe fn scan_avx512(masks: &[u64], need: u64, base: u32, out: &mut Vec<u32>) {
    use core::arch::x86_64::*;

    let n = masks.len();
    let mut i = 0;
    let vneed = _mm512_set1_epi64(need as i64);
    let ptr = masks.as_ptr();

    while i + 8 <= n {
        // SAFETY: `i + 8 <= n`, so 64 bytes from `ptr.add(i)` are in-bounds.
        // `loadu` accepts unaligned addresses. AMD Zen 4 does not throttle 512-bit.
        let v = unsafe { _mm512_loadu_si512(ptr.add(i).cast()) };
        let vand = _mm512_and_si512(v, vneed);
        let mut bits = _mm512_cmpeq_epi64_mask(vand, vneed);
        while bits != 0 {
            let lane = bits.trailing_zeros() as usize;
            out.push(base.saturating_add((i + lane) as u32));
            bits &= bits.wrapping_sub(1);
        }
        i += 8;
    }
    while i < n {
        // SAFETY: `i < n`.
        let m = unsafe { *ptr.add(i) };
        if m & need == need {
            out.push(base.saturating_add(i as u32));
        }
        i += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(masks: &[u64], need: u64, base: u32) -> Vec<u32> {
        let mut out = Vec::new();
        scan_mask(masks, need, base, &mut out);
        out
    }

    fn collect_scalar(masks: &[u64], need: u64, base: u32) -> Vec<u32> {
        let mut out = Vec::new();
        scan_scalar(masks, need, base, &mut out);
        out
    }

    #[test]
    fn mask_letters_and_digits() {
        assert_eq!(mask_name(b"hello"), mask_name(b"HELLO"));
        let need = needle_mask(&["hlo"]);
        assert_eq!(mask_name(b"hello") & need, need);
        assert_ne!(mask_name(b"ho") & need, need);
        assert_eq!(
            mask_name(b"mp3") & needle_mask(&["mp3"]),
            needle_mask(&["mp3"])
        );
    }

    #[test]
    fn scan_matches_scalar_at_boundaries() {
        for &len in &[
            0usize, 1, 3, 4, 5, 7, 8, 9, 15, 16, 17, 31, 32, 33, 63, 64, 65,
        ] {
            let mut masks = vec![0u64; len];
            for (i, slot) in masks.iter_mut().enumerate() {
                *slot = if i % 3 == 0 {
                    mask_name(b"hello")
                } else {
                    mask_name(b"xyz")
                };
            }
            let need = needle_mask(&["hlo"]);
            assert_eq!(
                collect(&masks, need, 10),
                collect_scalar(&masks, need, 10),
                "len={len}"
            );
            assert_eq!(
                collect(&masks, 0, 3),
                collect_scalar(&masks, 0, 3),
                "need0 len={len}"
            );
        }
    }

    #[test]
    fn scan_all_zero_and_all_match() {
        let zeros = vec![0u64; 40];
        let all = vec![u64::MAX; 40];
        let need = needle_mask(&["abc"]);
        assert!(collect(&zeros, need, 0).is_empty());
        assert_eq!(collect(&all, need, 5).len(), 40);
        assert_eq!(collect(&all, need, 5)[0], 5);
    }
}
