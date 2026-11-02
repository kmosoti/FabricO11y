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

namespace admission_kernel

def SlotValid (s : Slot) : Prop :=
  s.len.val ≤ s.capacity.val ∧
  ((s.phase = Phase.Free ∨ s.phase = Phase.Reserved) → s.len.val = 0)

def PhaseCredits (p : Phase) : Nat × Nat × Nat × Nat :=
  match p with
  | .Free => (1, 0, 0, 0)
  | .Reserved => (0, 1, 0, 0)
  | .Ready => (0, 0, 1, 0)
  | .Leased => (0, 0, 0, 1)

theorem slot_ownership_conservation (s : Slot) :
    let c := PhaseCredits s.phase
    c.1 + c.2.1 + c.2.2.1 + c.2.2.2 = 1 := by
  cases s.phase <;> rfl

theorem pool_credit_conservation (slots : List Slot) :
    (slots.map (fun s => let c := PhaseCredits s.phase
      c.1 + c.2.1 + c.2.2.1 + c.2.2.2)).sum = slots.length := by
  induction slots with
  | nil => rfl
  | cons s tail ih =>
    simp only [List.map_cons, List.sum_cons, List.length_cons]
    rw [ih]
    have credit := slot_ownership_conservation s
    omega

theorem slot_construction_contract (id capacity : Usize) :
    Slot.new id capacity ⦃ s => SlotValid s ∧ s.id = id ∧ s.capacity = capacity ∧ s.phase = .Free ⦄ := by
  simp [Slot.new, SlotValid]

theorem slot_observers_contract (s : Slot) :
    (Slot.impl.id s ⦃ id => id = s.id ⦄) ∧
    (Slot.impl.capacity s ⦃ capacity => capacity = s.capacity ⦄) ∧
    (Slot.encoded_len s ⦃ len => len = s.len ⦄) := by
  simp [Slot.impl.id, Slot.impl.capacity, Slot.encoded_len]

theorem slot_reserve_contract (s : Slot) (h : SlotValid s) :
    Slot.reserve s ⦃ r => SlotValid r.2 ∧ r.2.id = s.id ∧ r.2.capacity = s.capacity ∧
      (r.1 = true ↔ s.phase = .Free) ∧ (r.1 = false → r.2 = s) ∧
      (r.1 = true → r.2.phase = .Reserved) ⦄ := by
  rcases s with ⟨id, capacity, len, phase⟩
  cases phase <;> simp_all [Slot.reserve, SlotValid]

theorem slot_publish_contract (s : Slot) (len : Usize) (h : SlotValid s) :
    Slot.publish s len ⦃ r => SlotValid r.2 ∧ r.2.id = s.id ∧ r.2.capacity = s.capacity ∧
      (r.1 = true ↔ s.phase = .Reserved ∧ len.val ≤ s.capacity.val) ∧
      (r.1 = false → r.2 = s) ∧
      (r.1 = true → r.2.phase = .Ready ∧ r.2.len = len) ⦄ := by
  rcases s with ⟨id, capacity, oldLen, phase⟩
  cases phase <;> simp_all [Slot.publish, SlotValid]
  step*

theorem slot_lease_contract (s : Slot) (h : SlotValid s) :
    Slot.lease s ⦃ r => SlotValid r.2 ∧ r.2.id = s.id ∧ r.2.capacity = s.capacity ∧
      r.2.len = s.len ∧
      (r.1.isSome = true ↔ s.phase = .Ready) ∧
      (r.1 = none → r.2 = s) ∧
      (∀ len, r.1 = some len → s.phase = .Ready ∧ r.2.phase = .Leased ∧ len = s.len) ⦄ := by
  rcases s with ⟨id, capacity, len, phase⟩
  cases phase <;> simp_all [Slot.lease, SlotValid]

theorem slot_cancel_contract (s : Slot) (h : SlotValid s) :
    Slot.cancel s ⦃ r => SlotValid r.2 ∧ r.2.id = s.id ∧ r.2.capacity = s.capacity ∧
      (r.1 = true ↔ s.phase = .Reserved) ∧ (r.1 = false → r.2 = s) ∧
      (r.1 = true → r.2.phase = .Free ∧ r.2.len.val = 0) ⦄ := by
  rcases s with ⟨id, capacity, len, phase⟩
  cases phase <;> simp_all [Slot.cancel, SlotValid]

theorem slot_release_contract (s : Slot) (h : SlotValid s) :
    Slot.release s ⦃ r => SlotValid r.2 ∧ r.2.id = s.id ∧ r.2.capacity = s.capacity ∧
      (r.1 = true ↔ s.phase = .Leased) ∧ (r.1 = false → r.2 = s) ∧
      (r.1 = true → r.2.phase = .Free ∧ r.2.len.val = 0) ⦄ := by
  rcases s with ⟨id, capacity, len, phase⟩
  cases phase <;> simp_all [Slot.release, SlotValid]

def FifoValid (s : Fifo) : Prop := s.head.val ≤ s.next.val

theorem fifo_construction_contract :
    Fifo.empty ⦃ s => FifoValid s ∧ s.head.val = 0 ∧ s.next.val = 0 ⦄ := by
  simp [Fifo.empty, FifoValid]

theorem fifo_enqueue_contract (s : Fifo) (h : FifoValid s) :
    Fifo.enqueue s ⦃ r => FifoValid r.2 ∧ r.2.head = s.head ∧
      (r.1 = none → r.2 = s ∧ s.next = core.num.U64.MAX) ∧
      (∀ ticket, r.1 = some ticket → ticket = s.next ∧ r.2.next.val = s.next.val + 1) ⦄ := by
  have bound := U64.le_max s.next
  unfold Fifo.enqueue
  step* <;> simp_all [FifoValid, U64.rMax, U64.max_eq] <;> omega

theorem fifo_dequeue_contract (s : Fifo) (ticket : U64) (h : FifoValid s) :
    Fifo.dequeue s ticket ⦃ r => FifoValid r.2 ∧ r.2.next = s.next ∧
      (r.1 = true ↔ s.head.val < s.next.val ∧ ticket = s.head) ∧
      (r.1 = false → r.2 = s) ∧
      (r.1 = true → r.2.head.val = s.head.val + 1) ⦄ := by
  unfold Fifo.dequeue
  step*; simp_all [FifoValid]

#print axioms slot_ownership_conservation
#print axioms pool_credit_conservation
#print axioms slot_construction_contract
#print axioms slot_observers_contract
#print axioms slot_reserve_contract
#print axioms slot_publish_contract
#print axioms slot_lease_contract
#print axioms slot_cancel_contract
#print axioms slot_release_contract
#print axioms fifo_enqueue_contract
#print axioms fifo_dequeue_contract
#print axioms fifo_construction_contract
end admission_kernel
