import Std.Tactic

/- This model covers byte credits only. It says nothing about allocation,
   concurrency, serialization, or the behavior of the Rust queue. -/
namespace Admission

def reserve (capacity used charge : Nat) : Option Nat :=
  if used ≤ capacity ∧ charge ≤ capacity - used then some (used + charge)
  else none

theorem reserve_preserves_bound (capacity used charge next : Nat)
    (h : reserve capacity used charge = some next) : next ≤ capacity := by
  unfold reserve at h
  split at h
  · rename_i guard
    have eq : used + charge = next := Option.some.inj h
    omega
  · contradiction

theorem reserve_rejects_overflow (capacity used charge : Nat)
    (h : capacity < used + charge) : reserve capacity used charge = none := by
  unfold reserve
  split
  · rename_i guard
    omega
  · rfl

#print axioms reserve_preserves_bound
#print axioms reserve_rejects_overflow
end Admission
