#![cfg(any(feature = "gemmology", fast_gemm))]
//! Cheat-proof parity for the accelerated int8 GEMM: gemmology (i8mm on ARM, AVX2
//! on x86) natively, and the pure-Rust wasm SIMD128 kernel on `wasm32 + simd128`.
//!
//! The accelerated GEMM must match the scalar [`ops::intgemm_affine`] — the kernel
//! already validated against the marian oracle (`tests/int8_parity.rs`,
//! `tests/ops_parity.rs`). Two independent kernels agreeing, with an external
//! oracle at the base, so the fast path is validated transitively without a
//! tautology. Covered shapes include the transformer's inner dims and output
//! widths that are *not* multiples of 8 (the shim's zero-padding path).
//!
//! Run natively with `cargo test`, and under a real wasm runtime with
//! `wasm-pack test --node` (RUSTFLAGS `-C target-feature=+simd128`) — the wasm run
//! proves the SIMD128 kernel is actually live, not silently scalar.

use fxtranslate::gemm::{self, PreparedB};
use fxtranslate::ops;

// Under wasm the tests run through `wasm-bindgen-test` (Node/browser runner);
// natively they are plain `#[test]`s. `wasm_test` applies both attributes so each
// test compiles and runs in either environment without duplicating the body.
#[cfg(target_arch = "wasm32")]
use wasm_bindgen_test::wasm_bindgen_test;

macro_rules! wasm_test {
    ($(#[$meta:meta])* fn $name:ident() $body:block) => {
        $(#[$meta])*
        #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
        #[cfg_attr(not(target_arch = "wasm32"), test)]
        fn $name() $body
    };
}

/// A small linear-congruential PRNG (deterministic, no dev-dependency).
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0
    }
    fn byte(&mut self) -> u8 {
        (self.next() >> 33) as u8
    }
    fn signed(&mut self) -> i8 {
        (self.next() >> 33) as i8
    }
    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32 * 2.0 - 1.0
    }
}

fn argmax_row(v: &[f32], row: usize, n: usize) -> usize {
    let r = &v[row * n..(row + 1) * n];
    (0..n)
        .max_by(|&a, &b| r[a].partial_cmp(&r[b]).unwrap())
        .unwrap()
}

/// Compare the SIMD kernel against the scalar reference for one shape, in the
/// numeric regime where every backend agrees exactly. Returns `true` if the SIMD
/// path actually ran, `false` if it was skipped (no kernel for this target, or `k`
/// not a multiple of the register width).
///
/// Weights are bounded to ±63 while activations span the full uint8 range. The
/// legacy SSE int8 kernels sum two `uint8 × int8` products into an int16 lane
/// (`maddubs`) before widening to int32, and that lane saturates; scalar, ARM,
/// wasm SIMD128 and the corrected AVX2 shifted-dot path accumulate exactly.
/// With `|weight| ≤ 63` the worst pair is `2 × 255 × 63 = 32130 < 32767`, so no
/// backend saturates and the comparison is exact on every arch. The saturating
/// regime (full-range weights) is characterized separately in
/// `full_range_matches_only_on_exact_backends`.
fn check(m: usize, k: usize, n: usize, seed: u64) -> bool {
    let mut r = Lcg(seed);
    let a: Vec<u8> = (0..m * k).map(|_| r.byte()).collect();
    // `i8 % 64` lands in [-63, 63] — see the maddubs int16 bound above.
    let b: Vec<i8> = (0..n * k).map(|_| r.signed() % 64).collect();
    let bias: Vec<f32> = (0..n).map(|_| r.unit() * 10.0).collect();
    let unquant = 0.000_7_f32;

    let scalar = ops::intgemm_affine(&a, m, k, &b, n, unquant, &bias);
    // No SIMD kernel compiled for this target (non-aarch64/x86_64, `portable`, or
    // no C++ compiler), or this `k` isn't a multiple of the register width: nothing
    // to compare against, so skip. `matches_scalar_across_shapes` enforces that not
    // *everything* skipped when a SIMD backend is required.
    let Some(prepared) = PreparedB::new(&b, n, k) else {
        eprintln!("skip m={m} k={k} n={n}: SIMD kernel did not prepare this shape");
        return false;
    };
    let gem = prepared.matmul(&a, m, unquant, &bias);

    assert_eq!(scalar.len(), gem.len(), "output length");
    let mut max_diff = 0.0f32;
    for (i, (&s, &g)) in scalar.iter().zip(&gem).enumerate() {
        let d = (s - g).abs();
        max_diff = max_diff.max(d);
        // Same integer accumulation and same unquant+bias formula → essentially
        // exact; allow a hair for f32 reassociation.
        assert!(
            d <= 1e-2 + 1e-4 * s.abs(),
            "value mismatch m={m} k={k} n={n} idx={i}: scalar={s} gemmology={g} diff={d}"
        );
    }
    // Greedy decode only reads the per-row argmax, so pin that exactly.
    for row in 0..m {
        assert_eq!(
            argmax_row(&scalar, row, n),
            argmax_row(&gem, row, n),
            "argmax mismatch m={m} k={k} n={n} row={row}"
        );
    }
    eprintln!(
        "ok m={m} k={k} n={n} max_diff={max_diff:.2e} backend={}",
        gemm::backend()
    );
    true
}

/// Set in CI (see .github/workflows/inference-rs.yml) to turn a silent scalar
/// fallback into a test failure: on a target we claim to accelerate, the SIMD
/// kernel must actually be compiled and exercised, not quietly skipped.
///
/// On wasm, environment variables don't thread through the `wasm-bindgen-test`
/// runner, so the gate becomes compile-time: this test only compiles at all when
/// `fast_gemm` is set (the crate gate at the top), and on wasm `fast_gemm` means
/// the SIMD128 kernel is in — so "require SIMD" is unconditionally true here. If
/// the wasm build had *not* enabled `simd128`, `backend()` would report `"scalar"`
/// and `simd_backend_is_live_when_required` below would fail, which is exactly the
/// cheat-proof guarantee we want.
#[cfg(target_arch = "wasm32")]
fn require_simd() -> bool {
    true
}

#[cfg(not(target_arch = "wasm32"))]
fn require_simd() -> bool {
    std::env::var_os("FXTRANSLATE_REQUIRE_SIMD").is_some()
}

/// The mirror of [`require_simd`]: set on the `portable` CI leg to assert the
/// build really fell back to the scalar kernel (no SIMD compiled), pinning the
/// "runs everywhere without a C++ toolchain" contract from the other direction.
fn require_scalar() -> bool {
    std::env::var_os("FXTRANSLATE_REQUIRE_SCALAR").is_some()
}

wasm_test! {
fn matches_scalar_across_shapes() {
    // Transformer-shaped inner dims (k = 384/1536) are multiples of 16/32/64, so
    // they run on i8mm, AVX2, and AVX-512 alike. k=16 only clears the NEON/SSE
    // register width, so it skips on AVX2 (32) — hence the "at least one ran" gate
    // below rather than "all ran".
    let ran = [
        check(1, 384, 32000, 1), // output projection, vocab-scale N (mult of 8)
        check(1, 384, 512, 2),   // single-row affine
        check(8, 384, 384, 3),   // batched, attention-shaped
        check(4, 1536, 384, 4),  // FFN second layer (k=1536)
        check(2, 384, 1536, 5),  // FFN first layer (n=1536)
        check(1, 384, 7, 6),     // tiny N, not a multiple of 8 → padding path
        check(3, 384, 251, 7),   // N not a multiple of 8, batched
        check(6, 16, 40, 8),     // minimal k (16): i8mm/SSE only
    ];

    if require_simd() {
        assert!(
            ran.iter().any(|&r| r),
            "FXTRANSLATE_REQUIRE_SIMD is set but every shape skipped the SIMD kernel \
             (backend={}) — the fast path silently fell back to scalar",
            gemm::backend()
        );
    }
}
}

// The cheat-proof gate: the parity above only means something if the "SIMD"
// kernel is a real SIMD kernel. When a backend is required, prove it isn't the
// scalar stub. Sourced from the compiled shim (xsimd's `Arch::name()`) natively,
// or reported as "wasm-simd128" by the wasm kernel — so it can't be faked from
// Rust.
wasm_test! {
fn simd_backend_is_live_when_required() {
    let backend = gemm::backend();
    eprintln!("gemm backend = {backend}");
    eprintln!("intra-op GEMM pool threads = {}", gemm::gemm_threads());
    if require_simd() {
        assert_ne!(
            backend, "scalar",
            "FXTRANSLATE_REQUIRE_SIMD is set but gemm::backend() == \"scalar\": \
             build.rs did not compile the SIMD shim for this target"
        );
    }
    if require_scalar() {
        assert_eq!(
            backend, "scalar",
            "FXTRANSLATE_REQUIRE_SCALAR is set but gemm::backend() == {backend:?}: \
             the portable build unexpectedly compiled a SIMD kernel"
        );
    }
    // Under wasm the only accelerated kernel is the pure-Rust SIMD128 one, so pin
    // the exact name: a green wasm run then *proves* `backend()=="wasm-simd128"`
    // (the assert message surfaces the actual value if it were anything else),
    // not merely that it isn't scalar.
    #[cfg(target_arch = "wasm32")]
    assert_eq!(
        backend, "wasm-simd128",
        "wasm build must run the SIMD128 kernel; got {backend:?} — was it built \
         without `-C target-feature=+simd128`?"
    );
}
}

/// Whether a backend accumulates int8 products through a saturating int16 lane
/// (`maddubs`) rather than straight into int32. These agree with the exact scalar
/// kernel only while the int16 lane doesn't overflow (see `check`); the exact
/// backends (ARM `usdot`, x86 VNNI `vpdpbusd`) agree on any input. Keep this in
/// sync with the table in gemm-backends.md.
fn backend_saturates(name: &str) -> bool {
    matches!(name, "ssse3" | "sse2")
}

// SayFlex regression: shifted activations span 0..254 in real inference, and
// admitted models contain weights above +/-63. Adjacent products must widen
// before addition can saturate an int16 lane. Include full uint8/int8 limits,
// several rows and a padded output width; all exact totals fit f32 integers.
wasm_test! {
fn avx2_full_range_shifted_dot_matches_exact_scalar() {
    let backend = gemm::backend();
    if backend != "avx2" {
        eprintln!("AVX2-specific regression not applicable to backend={backend}");
        return;
    }
    let (m, k, n) = (5, 384, 13);
    let a = [255u8, 254, 130, 128, 0].into_iter()
        .flat_map(|value| std::iter::repeat_n(value, k)).collect::<Vec<_>>();
    let b = (0..n).flat_map(|column| {
        let weight = [127i8, -128, 100, -100, 64, -64, 0][column % 7];
        std::iter::repeat_n(weight, k)
    }).collect::<Vec<_>>();
    let bias = vec![0.0; n];
    let scalar = ops::intgemm_affine(&a, m, k, &b, n, 1.0, &bias);
    let prepared = PreparedB::new(&b, n, k).expect("AVX2 kernel must accept aligned inner width");
    let simd = prepared.matmul(&a, m, 1.0, &bias);
    assert_eq!(simd, scalar, "AVX2 full-range integer dot product must not saturate");
}
}

// Characterize the saturating vs. exact split on *full-range* inputs (the regime
// `check` deliberately avoids). AVX2 used to be exempt here despite full-range
// model weights. The corrected shifted-dot path must match scalar just like
// ARM and this crate's wasm-simd128 kernel. Only uncorrected legacy SSE backends
// are reported without a match requirement.
wasm_test! {
fn full_range_matches_only_on_exact_backends() {
    let (m, k, n) = (1, 384, 32000); // vocab-scale output projection: saturation is easy to hit
    let mut r = Lcg(0xF017);
    let a: Vec<u8> = (0..m * k).map(|_| r.byte()).collect();
    let b: Vec<i8> = (0..n * k).map(|_| r.signed()).collect(); // full [-128,127], unbounded
    let bias: Vec<f32> = (0..n).map(|_| r.unit() * 10.0).collect();
    let unquant = 0.000_7_f32;

    let scalar = ops::intgemm_affine(&a, m, k, &b, n, unquant, &bias);
    let Some(prepared) = PreparedB::new(&b, n, k) else {
        eprintln!("skip: no SIMD backend built for this target");
        return;
    };
    let gem = prepared.matmul(&a, m, unquant, &bias);

    let max_diff = scalar
        .iter()
        .zip(&gem)
        .map(|(&s, &g)| (s - g).abs())
        .fold(0.0f32, f32::max);
    let backend = gemm::backend();
    eprintln!(
        "full-range max_diff={max_diff:.3e} backend={backend} (saturates={})",
        backend_saturates(backend)
    );

    if !backend_saturates(backend) {
        // Exact backend (usdot / VNNI / scalar): must match even on adversarial data.
        assert!(
            max_diff <= 1e-2 + 1e-4 * scalar.iter().map(|s| s.abs()).fold(0.0, f32::max),
            "exact backend {backend} diverged from scalar on full-range inputs (max_diff={max_diff})"
        );
    }
    // Saturating backend: divergence here is expected and documented, not a failure.
}
}

/// Intra-op GEMM pool (feature `gemm-threads`, Option A) — cross-arch validation.
///
/// With `FXT_GEMM_THREADS>1` the vocab-scale projection shape is above the shim's
/// work threshold, so `gemmology_multiply` splits its output columns across the
/// persistent pool. The column split is disjoint (no cross-thread reduction), so
/// the pooled result must stay bit-identical to the scalar reference — the same
/// gate as the sequential kernel. CI runs this on both SIMD arches (i8mm, AVX2)
/// with the pool active, so a green run proves the pool is correct there, not just
/// on the author's machine.
#[cfg(feature = "gemm-threads")]
#[test]
fn intra_op_pool_matches_scalar() {
    let threads = gemm::gemm_threads();
    eprintln!(
        "intra-op GEMM pool: threads={threads} backend={}",
        gemm::backend()
    );
    // m·k·n well above the shim's parallel threshold, so the pool engages when
    // FXT_GEMM_THREADS>1; `check` asserts the result matches the scalar kernel.
    let ran = check(4, 384, 32000, 99);
    if require_simd() {
        assert!(
            ran,
            "SIMD required but the vocab-scale pool shape skipped (backend={})",
            gemm::backend()
        );
        // The pool is opt-in via FXT_GEMM_THREADS; when CI sets it >1 this proves
        // the intra-op path was live on this arch rather than silently sequential.
        if let Ok(n) = std::env::var("FXT_GEMM_THREADS") {
            if n.parse::<usize>().unwrap_or(1) > 1 {
                assert!(
                    threads > 1,
                    "FXT_GEMM_THREADS={n} but the shim pool reports {threads} threads"
                );
            }
        }
    }
}

wasm_test! {
fn rejects_k_not_multiple_of_register_width() {
    // The int8 register is 16 (i8mm/SSE), 32 (AVX2), or 64 (AVX-512) wide; k=24 is
    // a multiple of none, so the wrapper reports None and the caller keeps the
    // scalar kernel regardless of which backend is compiled.
    assert!(PreparedB::new(&vec![0i8; 3 * 24], 3, 24).is_none());
}
}
