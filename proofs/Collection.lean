import Std.Tactic

/- Pure models. Atomic storage and the mapping from Rust buffers to these counts
   are assumptions, not verified implementations of SQLite or synchronization. -/
namespace Collection

structure Window where
  accepted : Nat
  confirmed : Nat
  slots : Nat

def Valid (s : Window) : Prop :=
  s.confirmed ≤ s.accepted ∧ s.accepted - s.confirmed ≤ s.slots

theorem admission_preserves_bound (s : Window) (h : Valid s)
    (room : s.accepted - s.confirmed < s.slots) :
    Valid { s with accepted := s.accepted + 1 } := by
  unfold Valid at *
  dsimp at *
  omega

theorem full_requires_refusal (s : Window) (h : Valid s)
    (full : s.accepted - s.confirmed = s.slots) :
    ¬ Valid { s with accepted := s.accepted + 1 } := by
  unfold Valid at *
  dsimp at *
  omega

theorem confirmation_preserves_bound (s : Window) (h : Valid s)
    (pending : s.confirmed < s.accepted) :
    Valid { s with confirmed := s.confirmed + 1 } := by
  unfold Valid at *
  dsimp at *
  omega

theorem fixed_frontier_stays_covered (target confirmed later : Nat)
    (done : target ≤ confirmed) (monotone : confirmed ≤ later) : target ≤ later := by
  omega

-- A byte-addressed, uncompressed, disjoint storage cost model. No allocator,
-- compressed representation, latency, RSS or global optimality claim follows.
theorem retained_bytes_bound (slots width retained : Nat) (h : retained ≤ slots) :
    retained * width ≤ slots * width := Nat.mul_le_mul_right width h

theorem full_payload_attains_bound (slots width reserved : Nat)
    (storage : slots * width ≤ reserved) :
    reserved - slots * width = 0 ↔ reserved = slots * width := by omega

def retainedBytes : List Nat → Nat
  | [] => 0
  | size :: rest => size + retainedBytes rest

theorem full_records_cost (count width : Nat) :
    retainedBytes (List.replicate count width) = count * width := by
  induction count with
  | zero => simp [retainedBytes]
  | succ count ih => simp [List.replicate_succ, retainedBytes, ih, Nat.succ_mul, Nat.add_comm]

theorem lossless_byte_storage_lower_bound (count width capacity : Nat)
    (stores : retainedBytes (List.replicate count width) ≤ capacity) :
    count * width ≤ capacity := by
  simpa [full_records_cost] using stores

structure Durable where
  records : Nat → Option Nat
  checkpoint : Nat

-- Payload Nat stands for exact byte-string identity. The engine atomically
-- publishes both fields on success. Failure and duplicate retries publish neither.
def submit (s : Durable) (key payload checkpoint : Nat) (commit : Bool) : Durable × Bool :=
  match s.records key with
  | some old => (s, old == payload)
  | none => if commit then
      ({ records := fun k => if k = key then some payload else s.records k,
         checkpoint := checkpoint }, true)
    else (s, false)

theorem duplicate_preserves_state (s : Durable) (k p c : Nat) (commit : Bool)
    (h : s.records k = some p) : submit s k p c commit = (s, true) := by
  simp [submit, h]

theorem conflict_preserves_state (s : Durable) (k p old c : Nat) (commit : Bool)
    (h : s.records k = some old) (different : old ≠ p) :
    submit s k p c commit = (s, false) := by
  simp [submit, h, different]

theorem abort_preserves_state (s : Durable) (k p c : Nat)
    (h : s.records k = none) : submit s k p c false = (s, false) := by
  simp [submit, h]

theorem commit_publishes_both (s : Durable) (k p c : Nat)
    (h : s.records k = none) :
    (submit s k p c true).1.records k = some p ∧
    (submit s k p c true).1.checkpoint = c := by
  simp [submit, h]

theorem receipt_implies_retention (s : Durable) (k p c : Nat) (commit : Bool)
    (ack : (submit s k p c commit).2 = true) :
    (submit s k p c commit).1.records k = some p := by
  cases h : s.records k with
  | none => cases commit <;> simp_all [submit]
  | some old => simp_all [submit]

#print axioms admission_preserves_bound
#print axioms full_requires_refusal
#print axioms confirmation_preserves_bound
#print axioms fixed_frontier_stays_covered
#print axioms retained_bytes_bound
#print axioms full_payload_attains_bound
#print axioms full_records_cost
#print axioms lossless_byte_storage_lower_bound
#print axioms duplicate_preserves_state
#print axioms conflict_preserves_state
#print axioms abort_preserves_state
#print axioms commit_publishes_both
#print axioms receipt_implies_retention
end Collection
