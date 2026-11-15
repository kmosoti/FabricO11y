import Std.Tactic

/- Mathematical proofs for multi-step Gorilla metric block compression,
   Euclidean 5-minute rollup bucket alignment, and durable watermark traces.
   These proofs establish properties of the mathematical model mirroring
   `fabric-core::ring` and `admission-kernel::telemetry`. -/
namespace Telemetry

structure BlockState where
  prevTs : Int
  prevDelta : Int
  prevBits : UInt64
  deriving DecidableEq

structure EncodedDelta where
  dod : Int
  xorBits : UInt64
  deriving DecidableEq

def encodeSample (st : BlockState) (sample : Int × UInt64) : EncodedDelta × BlockState :=
  let delta := sample.1 - st.prevTs
  let dod := delta - st.prevDelta
  let xorBits := st.prevBits ^^^ sample.2
  ({ dod := dod, xorBits := xorBits },
   { prevTs := sample.1, prevDelta := delta, prevBits := sample.2 })

def decodeSample (st : BlockState) (enc : EncodedDelta) : (Int × UInt64) × BlockState :=
  let delta := st.prevDelta + enc.dod
  let ts := st.prevTs + delta
  let bits := st.prevBits ^^^ enc.xorBits
  ((ts, bits), { prevTs := ts, prevDelta := delta, prevBits := bits })

def encodeStream : BlockState → List (Int × UInt64) → List EncodedDelta
  | _, [] => []
  | st, s :: rest =>
      let res := encodeSample st s
      res.1 :: encodeStream res.2 rest

def decodeStream : BlockState → List EncodedDelta → List (Int × UInt64)
  | _, [] => []
  | st, e :: rest =>
      let res := decodeSample st e
      res.1 :: decodeStream res.2 rest

private theorem uint64_xor_cancel (a b : UInt64) : a ^^^ (a ^^^ b) = b := by
  have h : a ^^^ (a ^^^ b) = (a ^^^ a) ^^^ b := (UInt64.xor_assoc a a b).symm
  rw [h, UInt64.xor_self, UInt64.zero_xor]

theorem gorilla_step_roundtrip (st : BlockState) (sample : Int × UInt64) :
    decodeSample st (encodeSample st sample).1 = (sample, (encodeSample st sample).2) := by
  rcases st with ⟨pt, pd, pv⟩
  rcases sample with ⟨ts, valBits⟩
  simp only [encodeSample, decodeSample]
  have h_delta : pd + (ts - pt - pd) = ts - pt := by omega
  have h_ts : pt + (ts - pt) = ts := by omega
  have h_bits : pv ^^^ (pv ^^^ valBits) = valBits := uint64_xor_cancel pv valBits
  simp [h_delta, h_ts, h_bits]

theorem gorilla_stream_roundtrip (samples : List (Int × UInt64)) (st : BlockState) :
    decodeStream st (encodeStream st samples) = samples := by
  induction samples generalizing st with
  | nil => rfl
  | cons s rest ih =>
      simp only [encodeStream, decodeStream]
      rw [gorilla_step_roundtrip st s]
      dsimp
      rw [ih]

def rollupBucketMs : Int := 300000

-- Lean 4 `Int` division `/` and `%` match Rust `div_euclid` / `rem_euclid`
-- for positive divisor `rollupBucketMs = 300000` over `[-9_223_372_036_854_600_000, i64::MAX]`.
-- Rust `align_rollup_bucket_ms` uses `.saturating_mul(300_000)`, so inputs in
-- `[i64::MIN, -9_223_372_036_854_600_001]` clamp to `i64::MIN` (`rem_euclid = 124_192 != 0`).
def alignRollup (ts : Int) : Int :=
  (ts / rollupBucketMs) * rollupBucketMs

theorem rollup_bucket_aligned (ts : Int) :
    alignRollup ts % rollupBucketMs = 0 := by
  unfold alignRollup rollupBucketMs
  exact Int.mul_emod_left (ts / 300000) 300000

theorem rollup_bucket_residual_bounds (ts : Int) :
    alignRollup ts ≤ ts ∧ ts < alignRollup ts + rollupBucketMs := by
  unfold alignRollup rollupBucketMs
  omega

theorem rollup_bucket_idempotent (ts : Int) :
    alignRollup (alignRollup ts) = alignRollup ts := by
  unfold alignRollup rollupBucketMs
  rw [Int.mul_ediv_cancel _ (by decide)]

theorem rollup_bucket_monotone (t1 t2 : Int) (h : t1 ≤ t2) :
    alignRollup t1 ≤ alignRollup t2 := by
  unfold alignRollup rollupBucketMs
  have hq : t1 / 300000 ≤ t2 / 300000 := Int.ediv_le_ediv (by decide) h
  exact Int.mul_le_mul_of_nonneg_right hq (by decide)

def watermarkStep (committed candidate : Nat) (flushOk : Bool) : Nat :=
  if flushOk && decide (candidate > committed) then candidate else committed

def watermarkTrace (initial : Nat) : List (Nat × Bool) → Nat
  | [] => initial
  | (cand, ok) :: rest => watermarkTrace (watermarkStep initial cand ok) rest

theorem watermark_failed_flush_unchanged (committed candidate : Nat) :
    watermarkStep committed candidate false = committed := by
  simp [watermarkStep]

theorem watermark_step_monotone (committed candidate : Nat) (ok : Bool) :
    committed ≤ watermarkStep committed candidate ok := by
  unfold watermarkStep
  split <;> simp_all <;> omega

theorem watermark_trace_monotone (initial : Nat) (steps : List (Nat × Bool)) :
    initial ≤ watermarkTrace initial steps := by
  induction steps generalizing initial with
  | nil => exact Nat.le_refl initial
  | cons step rest ih =>
      simp only [watermarkTrace]
      have h1 := watermark_step_monotone initial step.1 step.2
      have h2 := ih (watermarkStep initial step.1 step.2)
      omega

#print axioms gorilla_step_roundtrip
#print axioms gorilla_stream_roundtrip
#print axioms rollup_bucket_aligned
#print axioms rollup_bucket_residual_bounds
#print axioms rollup_bucket_idempotent
#print axioms rollup_bucket_monotone
#print axioms watermark_failed_flush_unchanged
#print axioms watermark_step_monotone
#print axioms watermark_trace_monotone
end Telemetry
