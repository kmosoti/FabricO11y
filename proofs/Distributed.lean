import Std.Tactic
namespace Distributed
-- An identity includes its persistent origin. Payload equality denotes exact
-- canonical-record equality, not an assumption about cryptographic hashes.
abbrev Identity := Nat × Nat × Nat × Nat
structure State where
  spindle : Identity → Option Nat
  fabric : Identity → Option Nat
  capacity : Nat
  pending : Nat
  seen : Identity → Bool := fun _ => false

def publish (store : Identity → Option Nat) (k : Identity) (p : Nat) :=
  match store k with
  | none => fun j => if j = k then some p else store j
  | some _ => store

abbrev SpindleSeal (s : State) (k : Identity) (p : Nat) := s.spindle k = some p
abbrev FabricSeal (s : State) (k : Identity) (p : Nat) := s.fabric k = some p
def handoff (s : State) (k : Identity) (p : Nat) : State :=
  if SpindleSeal s k p then {s with fabric := publish s.fabric k p} else s

theorem local_is_not_remote : ∃ s k p, SpindleSeal s k p ∧ ¬ FabricSeal s k p := by
  exact ⟨{spindle := fun _ => some 1, fabric := fun _ => none, capacity := 1, pending := 1}, (0,0,0,0), 1, rfl, by simp [FabricSeal]⟩

theorem publish_retains (store : Identity → Option Nat) (k : Identity) (p : Nat)
    (empty : store k = none) : publish store k p k = some p := by simp [publish, empty]

theorem conflict_cannot_overwrite (store : Identity → Option Nat) (k : Identity) (p old : Nat)
    (hfound : store k = some old) : publish store k p = store := by simp [publish, hfound]

theorem duplicates_idempotent (store : Identity → Option Nat) (k : Identity) (p : Nat)
    (hfound : store k = some p) : publish store k p = store := by simp [publish, hfound]

theorem other_origin_unchanged (store : Identity → Option Nat) (k j : Identity) (p : Nat)
    (distinct : j ≠ k) : publish store k p j = store j := by
  unfold publish
  cases store k <;> simp [distinct]

theorem local_custody_survives_handoff (s : State) (k : Identity) (p : Nat) :
    (handoff s k p).spindle = s.spindle := by unfold handoff; split <;> rfl

theorem delivered_empty_server_seals (s : State) (k : Identity) (p : Nat)
    (hlocal : SpindleSeal s k p) (empty : s.fabric k = none) :
    FabricSeal (handoff s k p) k p := by
  unfold FabricSeal
  rw [handoff, ite_eq_left hlocal]
  exact publish_retains s.fabric k p empty

def Prefix (stored : Nat → Bool) (frontier : Nat) := ∀ n, 1 ≤ n → n ≤ frontier → stored n = true

theorem frontier_cannot_skip (stored : Nat → Bool) (f missing : Nat)
    (hprefix : Prefix stored f) (positive : 1 ≤ missing) (absent : stored missing = false) : f < missing := by
  by_cases le : missing ≤ f
  · have bad : false = true := absent.symm.trans (hprefix missing positive le)
    cases bad
  · omega

theorem frontier_advance (stored : Nat → Bool) (f : Nat)
    (hprefix : Prefix stored f) (next : stored (f+1) = true) : Prefix stored (f+1) := by
  intro n positive bound
  by_cases eq : n = f+1
  · simpa [eq] using next
  · exact hprefix n positive (by omega)

def Reclaimable (s : State) (k : Identity) (p : Nat) := SpindleSeal s k p ∧ FabricSeal s k p

theorem reclamation_requires_remote (s : State) (k : Identity) (p : Nat)
    (h : Reclaimable s k p) : FabricSeal s k p := h.2

theorem bounded_enqueue (pending capacity : Nat) (room : pending < capacity) : pending+1 ≤ capacity := by omega

theorem full_refuses (pending capacity : Nat) (full : pending = capacity) : ¬ pending+1 ≤ capacity := by omega

-- Durable contents survive arbitrary traces of process/network failures and
-- handoffs under the atomic-storage model. No spontaneous deletion transition.
theorem publish_preserves_existing (store : Identity → Option Nat) (k j : Identity) (p old : Nat)
    (h : store j = some old) : publish store k p j = some old := by
  by_cases same : j = k
  · subst j
    simp [publish, h]
  · cases hk : store k <;> simp [publish, hk, same, h]

inductive Event where
  | dropMessage
  | restart
  | deliver (key : Identity) (payload : Nat)

def step (s : State) : Event → State
  | .dropMessage => s
  | .restart => s
  | .deliver k p => handoff s k p

theorem step_preserves_remote (s : State) (event : Event) (j : Identity) (old : Nat)
    (h : FabricSeal s j old) : FabricSeal (step s event) j old := by
  cases event with
  | dropMessage => exact h
  | restart => exact h
  | deliver k p =>
    change (handoff s k p).fabric j = some old
    unfold handoff
    split
    · exact publish_preserves_existing s.fabric k j p old h
    · exact h

def trace (s : State) : List Event → State
  | [] => s
  | e :: es => trace (step s e) es

theorem arbitrary_trace_retains_remote (events : List Event) (s : State) (j : Identity) (old : Nat)
    (h : FabricSeal s j old) : FabricSeal (trace s events) j old := by
  induction events generalizing s with
  | nil => exact h
  | cons e es ih => exact ih (step s e) (step_preserves_remote s e j old h)

theorem lost_ack_can_be_durable : ∃ s k p, FabricSeal s k p ∧ s.seen k = false := by
  exact ⟨{spindle := fun _ => some 1, fabric := fun _ => some 1, capacity := 1, pending := 1}, (0,0,0,0), 1, rfl, rfl⟩

def observe (s : State) (k : Identity) (p : Nat) : State :=
  if FabricSeal s k p then {s with seen := fun j => if j = k then true else s.seen j} else s

theorem unknown_ack_does_not_advance (s : State) (k : Identity) (p : Nat)
    (absent : ¬ FabricSeal s k p) : observe s k p = s := by simp [observe, absent]

-- A fair retry schedule must establish one successful removal at each step;
-- the rank then decreases and cannot require more than the initial pending count.
def drain : Nat → Nat → Nat
  | n, 0 => n
  | n, fuel+1 => drain (n-1) fuel

theorem finite_drain (pending : Nat) : drain pending pending = 0 := by
  induction pending with
  | zero => rfl
  | succ n ih => simpa [drain] using ih

#print axioms publish_preserves_existing
#print axioms step_preserves_remote
#print axioms arbitrary_trace_retains_remote
#print axioms lost_ack_can_be_durable
#print axioms unknown_ack_does_not_advance
#print axioms local_is_not_remote
#print axioms publish_retains
#print axioms conflict_cannot_overwrite
#print axioms duplicates_idempotent
#print axioms other_origin_unchanged
#print axioms local_custody_survives_handoff
#print axioms delivered_empty_server_seals
#print axioms frontier_cannot_skip
#print axioms frontier_advance
#print axioms reclamation_requires_remote
#print axioms bounded_enqueue
#print axioms full_refuses
#print axioms finite_drain
end Distributed
