import AdmissionKernel
open Aeneas Aeneas.Std Result
namespace admission_kernel

theorem reserve_preserves_bound (capacity used charge : Usize)
    (hu : used.val ≤ capacity.val)
    (hc : charge.val ≤ capacity.val - used.val) :
    reserve capacity used charge ⦃ result =>
      ∃ next, result = some next ∧ next.val = used.val + charge.val ∧ next.val ≤ capacity.val ⦄ := by
  unfold reserve
  step*

theorem reserve_rejects_invalid_state (capacity used charge : Usize)
    (hu : capacity.val < used.val) :
    reserve capacity used charge = ok none := by
  have h : ¬ used ≤ capacity := by scalar_tac
  simp [reserve, h]

theorem reserve_rejects_excess_charge (capacity used charge : Usize)
    (hu : used.val ≤ capacity.val)
    (hc : capacity.val - used.val < charge.val) :
    reserve capacity used charge ⦃ result => result = none ⦄ := by
  unfold reserve
  step*

#print axioms reserve_preserves_bound
#print axioms reserve_rejects_invalid_state
#print axioms reserve_rejects_excess_charge
end admission_kernel
