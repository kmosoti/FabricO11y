; SMT-LIB2 bit-exact verification for FabricO11y converged telemetry primitives
; Checked with pinned Z3 (v4.16.0) via `cargo xtask prove smt`.
(set-logic QF_BV)

(define-fun zigzag_encode ((x (_ BitVec 64))) (_ BitVec 64)
  (bvxor (bvshl x #x0000000000000001) (bvashr x #x000000000000003f)))

(define-fun zigzag_decode ((z (_ BitVec 64))) (_ BitVec 64)
  (bvxor (bvlshr z #x0000000000000001) (bvneg (bvand z #x0000000000000001))))

; Property 1: 64-bit ZigZag encode/decode form an exact total bijection.
(push 1)
(declare-const x (_ BitVec 64))
(declare-const z (_ BitVec 64))
(assert (or
  (distinct (zigzag_decode (zigzag_encode x)) x)
  (distinct (zigzag_encode (zigzag_decode z)) z)))
(check-sat)
(pop 1)

; Property 2: Gorilla two's-complement modular delta-of-delta timestamp encoding
; roundtrips arbitrary (including out-of-order and extreme i64) timestamps.
(push 1)
(declare-const t0 (_ BitVec 64))
(declare-const t1 (_ BitVec 64))
(declare-const t2 (_ BitVec 64))
(define-fun enc_d1 () (_ BitVec 64) (bvsub t1 t0))
(define-fun dod1 () (_ BitVec 64) (bvsub enc_d1 #x0000000000000000))
(define-fun enc_d2 () (_ BitVec 64) (bvsub t2 t1))
(define-fun dod2 () (_ BitVec 64) (bvsub enc_d2 enc_d1))
(define-fun dec_d1 () (_ BitVec 64) (bvadd #x0000000000000000 dod1))
(define-fun dec_t1 () (_ BitVec 64) (bvadd t0 dec_d1))
(define-fun dec_d2 () (_ BitVec 64) (bvadd dec_d1 dod2))
(define-fun dec_t2 () (_ BitVec 64) (bvadd dec_t1 dec_d2))
(assert (or (distinct dec_t1 t1) (distinct dec_t2 t2)))
(check-sat)
(pop 1)

; Property 3: IEEE-754 64-bit XOR delta encoding with little-endian zero-byte trimming
; reconstructs arbitrary f64 bit patterns (including -0.0, +/-inf, and NaN payloads).
(push 1)
(declare-const v0 (_ BitVec 64))
(declare-const v1 (_ BitVec 64))
(define-fun xor_diff () (_ BitVec 64) (bvxor v0 v1))
(define-fun trimmed_xor () (_ BitVec 64)
  (ite (= xor_diff #x0000000000000000) #x0000000000000000
  (ite (= ((_ extract 63 8) xor_diff) (_ bv0 56)) (bvand xor_diff #x00000000000000ff)
  (ite (= ((_ extract 63 16) xor_diff) (_ bv0 48)) (bvand xor_diff #x000000000000ffff)
  (ite (= ((_ extract 63 24) xor_diff) (_ bv0 40)) (bvand xor_diff #x0000000000ffffff)
  (ite (= ((_ extract 63 32) xor_diff) (_ bv0 32)) (bvand xor_diff #x00000000ffffffff)
  (ite (= ((_ extract 63 40) xor_diff) (_ bv0 24)) (bvand xor_diff #x000000ffffffffff)
  (ite (= ((_ extract 63 48) xor_diff) (_ bv0 16)) (bvand xor_diff #x0000ffffffffffff)
  (ite (= ((_ extract 63 56) xor_diff) (_ bv0 8)) (bvand xor_diff #x00ffffffffffffff)
    xor_diff)))))))))
(define-fun dec_v1 () (_ BitVec 64) (bvxor v0 trimmed_xor))
(assert (distinct dec_v1 v1))
(check-sat)
(pop 1)

; Property 4: Durable Spindle watermark progression is monotonic and never advances
; when flush_succeeded is false.
(push 1)
(declare-const committed (_ BitVec 64))
(declare-const candidate (_ BitVec 64))
(declare-const flush_ok Bool)
(define-fun next_wm () (_ BitVec 64)
  (ite (and flush_ok (bvugt candidate committed)) candidate committed))
(assert (or
  (bvult next_wm committed)
  (and (not flush_ok) (distinct next_wm committed))
  (and flush_ok (bvugt candidate committed) (distinct next_wm candidate))
  (and flush_ok (bvule candidate committed) (distinct next_wm committed))))
(check-sat)
(pop 1)
