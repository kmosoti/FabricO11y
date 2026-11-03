import Std.Tactic

/- Independent protocol model. Bytes are compared exactly, not by a hash.
   Atomic durable reservation/publication, truthful authenticated receipts,
   exact engine readback, and eventual worker cleanup are refinement assumptions.
   This file does not model SQL, filesystems, cryptography, or runtime scheduling. -/
namespace Custody
abbrev Key := Nat × Nat -- persistent origin, positive source sequence
abbrev Bytes := List UInt8
structure Record where
  id : Nat
  identity : Nat -- injective abstraction of the producer/generation/sequence tuple
  bytes : Bytes
  deriving DecidableEq
structure Packet where
  key : Key
  record : Record
structure State where
  binding : Key → Option Record
  owner : (Nat × Nat) → Option Key -- origin/retry identity permanently binds one source key
  pending : Key → Bool
  engine : Nat → Option Record
  visible : Key → Bool
  frontier : Nat → Nat
  allocated : Nat

def reserve (s : State) (key : Key) (identity : Nat) (bytes : Bytes) : State :=
  match s.binding key with
  | some _ => s
  | none =>
    if s.owner (key.1,identity) = none then
      let r : Record := ⟨s.allocated + 1, identity, bytes⟩
      {s with
        binding := fun k => if k = key then some r else s.binding k
        owner := fun i => if i = (key.1,identity) then some key else s.owner i
        pending := fun k => if k = key then true else s.pending k
        allocated := s.allocated + 1}
    else s

-- Packets can finish arbitrarily late. Only the exact durable reservation can
-- authorize an upsert; published bindings are not removed when pending clears.
def engineWrite (s : State) (packet : Packet) : State :=
  if s.binding packet.key = some packet.record then
    {s with engine := fun id => if id = packet.record.id then some packet.record else s.engine id}
  else s

-- An unknown network result can conceal either outcome. Neither branch frees
-- the pending reservation or reallocates its ID.
def unknown (s : State) (packet : Packet) (committed : Bool) : State :=
  if committed then engineWrite s packet else s

def publish (s : State) (key : Key) : State :=
  match s.binding key with
  | none => s
  | some r =>
    if s.pending key = true ∧ s.engine r.id = some r ∧ key.2 = s.frontier key.1 + 1 then
      {s with
        pending := fun k => if k = key then false else s.pending k
        visible := fun k => if k = key then true else s.visible k
        frontier := fun origin => if origin = key.1 then key.2 else s.frontier origin}
    else s

theorem existing_binding_immutable (s : State) (key other : Key) (identity : Nat)
    (bytes : Bytes) (old : Record) (bound : s.binding other = some old) :
    (reserve s key identity bytes).binding other = some old := by
  unfold reserve
  cases h : s.binding key with
  | some r => exact bound
  | none =>
    simp only
    split
    · have different : other ≠ key := by intro equal; subst other; simp_all
      simp [different, bound]
    · exact bound

theorem retry_identity_cannot_move_to_new_source (s : State) (key oldKey : Key)
    (identity : Nat) (bytes : Bytes) (unbound : s.binding key = none)
    (owned : s.owner (key.1,identity) = some oldKey) : reserve s key identity bytes = s := by
  simp [reserve, unbound, owned]

theorem conflicting_retry_refused (s : State) (key : Key) (identity : Nat)
    (bytes : Bytes) (old : Record) (bound : s.binding key = some old)
    (_conflict : old.identity ≠ identity ∨ old.bytes ≠ bytes) :
    reserve s key identity bytes = s := by simp [reserve, bound]

theorem reservation_precedes_upsert (s : State) (packet : Packet)
    (changed : (engineWrite s packet).engine packet.record.id ≠ s.engine packet.record.id) :
    s.binding packet.key = some packet.record := by
  unfold engineWrite at changed
  split at changed
  · assumption
  · exact False.elim (changed rfl)

theorem unknown_retains_reservation_and_ids (s : State) (packet : Packet) (committed : Bool) :
    (unknown s packet committed).binding = s.binding ∧
    (unknown s packet committed).pending = s.pending ∧
    (unknown s packet committed).allocated = s.allocated := by
  cases committed <;> simp [unknown, engineWrite] <;> split <;> simp

theorem fresh_id_separates_old (s : State) (old : Record)
    (within : old.id ≤ s.allocated) : old.id ≠ s.allocated + 1 := by omega

theorem delayed_packet_preserves_fresh_id (s : State) (packet : Packet)
    (old : packet.record.id ≤ s.allocated) :
    (engineWrite s packet).engine (s.allocated + 1) = s.engine (s.allocated + 1) := by
  unfold engineWrite
  split
  · have different : s.allocated + 1 ≠ packet.record.id := by omega
    simp [different]
  · rfl

theorem missing_readback_blocks_publication (s : State) (key : Key) (r : Record)
    (bound : s.binding key = some r) (missing : s.engine r.id ≠ some r) :
    publish s key = s := by simp [publish, bound, missing]

theorem publication_requires_full_readback (s : State) (key : Key)
    (wasHidden : s.visible key = false) (nowVisible : (publish s key).visible key = true) :
    ∃ r, s.binding key = some r ∧ s.pending key = true ∧ s.engine r.id = some r := by
  unfold publish at nowVisible
  cases h : s.binding key with
  | none => simp [h, wasHidden] at nowVisible
  | some r =>
    simp only [h] at nowVisible
    split at nowVisible
    · rename_i gate
      exact ⟨r, rfl, gate.1, gate.2.1⟩
    · simp [wasHidden] at nowVisible

def Prefix (s : State) : Prop :=
  ∀ origin n, 0 < n → n ≤ s.frontier origin → s.visible (origin,n) = true

theorem publication_never_skips_source (s : State) (key : Key)
    (hprefix : Prefix s) : Prefix (publish s key) := by
  unfold publish
  cases h : s.binding key with
  | none => exact hprefix
  | some r =>
    simp only
    split
    · rename_i gate
      intro origin n positive bounded
      dsimp at bounded ⊢
      by_cases sameOrigin : origin = key.1
      · subst origin
        simp only [ite_true] at bounded
        by_cases sameSource : n = key.2
        · have eq : (key.1,n) = key := by cases key; simp_all
          simp [eq]
        · have oldBound : n ≤ s.frontier key.1 := by omega
          have old := hprefix key.1 n positive oldBound
          have neq : (key.1,n) ≠ key := by intro eq; exact sameSource (congrArg Prod.snd eq)
          simp [neq, old]
      · have oldBound : n ≤ s.frontier origin := by simpa [sameOrigin] using bounded
        have neq : (origin,n) ≠ key := by intro eq; exact sameOrigin (congrArg Prod.fst eq)
        simp [neq, hprefix origin n positive oldBound]
    · exact hprefix

theorem publish_keeps_binding (s : State) (key : Key) :
    (publish s key).binding = s.binding := by
  unfold publish
  split
  · rfl
  · split <;> rfl

inductive Event where
  | reserve (key : Key) (identity : Nat) (bytes : Bytes)
  | write (packet : Packet)
  | unknown (packet : Packet) (committed : Bool)
  | publish (key : Key)

def step (s : State) : Event → State
  | .reserve key identity bytes => reserve s key identity bytes
  | .write packet => engineWrite s packet
  | .unknown packet committed => unknown s packet committed
  | .publish key => publish s key

def trace (s : State) : List Event → State
  | [] => s
  | event :: rest => trace (step s event) rest

theorem step_preserves_binding (s : State) (event : Event) (key : Key)
    (old : Record) (bound : s.binding key = some old) :
    (step s event).binding key = some old := by
  cases event with
  | reserve k identity bytes => exact existing_binding_immutable s k key identity bytes old bound
  | write packet => simp [step, engineWrite]; split <;> exact bound
  | unknown packet committed =>
    have h := (unknown_retains_reservation_and_ids s packet committed).1
    change (unknown s packet committed).binding key = some old
    rw [h]; exact bound
  | publish k =>
    change (publish s k).binding key = some old
    rw [publish_keeps_binding]
    exact bound

theorem arbitrary_trace_preserves_binding (events : List Event) (s : State)
    (key : Key) (old : Record) (bound : s.binding key = some old) :
    (trace s events).binding key = some old := by
  induction events generalizing s with
  | nil => exact bound
  | cons event rest ih => exact ih (step s event) (step_preserves_binding s event key old bound)

theorem reserve_assigns_fresh_record (s : State) (key : Key) (identity : Nat) (bytes : Bytes)
    (empty : s.binding key = none) (unowned : s.owner (key.1,identity) = none) :
    (reserve s key identity bytes).binding key = some ⟨s.allocated+1,identity,bytes⟩ ∧
    (reserve s key identity bytes).allocated = s.allocated+1 ∧
    (reserve s key identity bytes).pending key = true := by simp [reserve, empty, unowned]

theorem engine_write_other_id (s : State) (packet : Packet) (id : Nat)
    (different : id ≠ packet.record.id) :
    (engineWrite s packet).engine id = s.engine id := by
  unfold engineWrite
  split <;> simp [different]

theorem delayed_exact_retry_preserves_new_bytes (s : State) (old : Packet) (key : Key)
    (identity : Nat) (bytes : Bytes) (empty : s.binding key = none)
    (unowned : s.owner (key.1,identity) = none) (oldId : old.record.id ≤ s.allocated) :
    let fresh : Record := ⟨s.allocated+1,identity,bytes⟩
    let written := engineWrite (reserve s key identity bytes) ⟨key,fresh⟩
    (engineWrite written old).engine fresh.id = some fresh := by
  dsimp
  rw [engine_write_other_id _ old (s.allocated+1) (by omega)]
  simp [engineWrite, reserve, empty, unowned]

def WithinAllocation (s : State) : Prop :=
  ∀ key r, s.binding key = some r → r.id ≤ s.allocated

theorem reservation_preserves_allocation_bound (s : State) (key : Key)
    (identity : Nat) (bytes : Bytes) (within : WithinAllocation s) :
    WithinAllocation (reserve s key identity bytes) := by
  unfold reserve
  cases h : s.binding key with
  | some r => exact within
  | none =>
    simp only
    split
    · intro other r bound
      dsimp at bound ⊢
      by_cases same : other = key
      · simp [same] at bound
        cases bound
        exact Nat.le_refl _
      · have previous : s.binding other = some r := by simpa [same] using bound
        have old := within other r previous
        omega
    · exact within

theorem step_preserves_allocation_bound (s : State) (event : Event)
    (within : WithinAllocation s) : WithinAllocation (step s event) := by
  cases event with
  | reserve key identity bytes => exact reservation_preserves_allocation_bound s key identity bytes within
  | write packet =>
    change WithinAllocation (engineWrite s packet)
    unfold engineWrite
    split <;> exact within
  | unknown packet committed =>
    cases committed with
    | false => exact within
    | true =>
      change WithinAllocation (engineWrite s packet)
      unfold engineWrite
      split <;> exact within
  | publish key =>
    change WithinAllocation (publish s key)
    unfold publish
    split
    · exact within
    · split <;> exact within

theorem trace_preserves_allocation_bound (events : List Event) (s : State)
    (within : WithinAllocation s) : WithinAllocation (trace s events) := by
  induction events generalizing s with
  | nil => exact within
  | cons event rest ih => exact ih (step s event) (step_preserves_allocation_bound s event within)

theorem step_preserves_prefix (s : State) (event : Event) (hprefix : Prefix s) :
    Prefix (step s event) := by
  cases event with
  | reserve key identity bytes =>
    change Prefix (reserve s key identity bytes)
    unfold reserve
    split
    · exact hprefix
    · split <;> exact hprefix
  | write packet =>
    change Prefix (engineWrite s packet)
    unfold engineWrite
    split <;> exact hprefix
  | unknown packet committed =>
    cases committed with
    | false => exact hprefix
    | true =>
      change Prefix (engineWrite s packet)
      unfold engineWrite
      split <;> exact hprefix
  | publish key => exact publication_never_skips_source s key hprefix

theorem arbitrary_trace_preserves_source_prefix (events : List Event) (s : State)
    (hprefix : Prefix s) : Prefix (trace s events) := by
  induction events generalizing s with
  | nil => exact hprefix
  | cons event rest ih => exact ih (step s event) (step_preserves_prefix s event hprefix)

def initial : State :=
  {binding := fun _ => none, owner := fun _ => none, pending := fun _ => false, engine := fun _ => none,
    visible := fun _ => false, frontier := fun _ => 0, allocated := 0}

theorem arbitrary_initial_trace_has_no_holes_or_unallocated_ids (events : List Event) :
    Prefix (trace initial events) ∧ WithinAllocation (trace initial events) := by
  constructor
  · apply arbitrary_trace_preserves_source_prefix events initial
    intro origin n positive bounded
    dsimp [initial] at bounded
    omega
  · apply trace_preserves_allocation_bound events initial
    intro key r bound
    simp [initial] at bound

-- Local payload deletion has an additional shadow-rollout gate. Receipt
-- provenance means authenticated durable authority; it is an external assumption.
structure Receipt where
  authority : Nat
  origin : Nat
  through : Nat
structure Local where
  authority : Nat
  origin : Nat
  confirmed : Nat
  optedIn : Bool
  payload : Nat → Bool

def reclaim (s : Local) (receipt : Receipt) (id : Nat) : Local :=
  if s.optedIn = true ∧ receipt.authority = s.authority ∧ receipt.origin = s.origin ∧
      0 < id ∧ id ≤ receipt.through ∧ receipt.through ≤ s.confirmed then
    {s with payload := fun n => if n = id then false else s.payload n}
  else s

theorem shadow_cannot_delete (s : Local) (receipt : Receipt) (id : Nat)
    (shadow : s.optedIn = false) : reclaim s receipt id = s := by simp [reclaim, shadow]

theorem gc_requires_matching_confirmation (s : Local) (receipt : Receipt) (id : Nat)
    (retained : s.payload id = true) (deleted : (reclaim s receipt id).payload id = false) :
    receipt.authority = s.authority ∧ receipt.origin = s.origin ∧
    0 < id ∧ id ≤ receipt.through ∧ receipt.through ≤ s.confirmed := by
  unfold reclaim at deleted
  split at deleted
  · rename_i gate
    exact gate.2
  · simp [retained] at deleted

inductive Lease where
  | v2
  | authenticated (expiry : Nat) (authentic : Bool)

def admitLease (lease : Lease) (now floor : Nat) (_metadataPresent : Bool) : Bool :=
  match lease with
  | .v2 => true
  | .authenticated expiry authentic => authentic && decide (floor ≤ now ∧ now < expiry)

theorem expiry_independent_of_metadata (expiry now floor : Nat) (authentic metadata : Bool)
    (expired : expiry ≤ now) : admitLease (.authenticated expiry authentic) now floor metadata = false := by
  simp [admitLease]; omega

theorem unauthenticated_lease_refused (expiry now floor : Nat) (metadata : Bool) :
    admitLease (.authenticated expiry false) now floor metadata = false := by simp [admitLease]

theorem legacy_not_retroactively_expired (now floor : Nat) (metadata : Bool) :
    admitLease .v2 now floor metadata = true := by rfl

structure Generation where
  expiry : Nat
  admitted : Nat
  forwarded : Nat
  deriving DecidableEq

def retire (g : Generation) (now : Nat) : Option Generation :=
  if g.expiry ≤ now ∧ g.admitted ≤ g.forwarded then none else some g

theorem retirement_requires_expiry_and_forwarding (g : Generation) (now : Nat)
    (removed : retire g now = none) : g.expiry ≤ now ∧ g.admitted ≤ g.forwarded := by
  unfold retire at removed; split at removed
  · assumption
  · simp at removed

theorem pending_generation_survives_expiry (g : Generation) (now : Nat)
    (pending : g.forwarded < g.admitted) : retire g now = some g := by simp [retire]; omega

structure Worker where
  held : Bool
  cancellationRequested : Bool
inductive WorkerEvent where
  | cancel
  | progress
  | cleanup

def workerStep (s : Worker) : WorkerEvent → Worker
  | .cancel => {s with cancellationRequested := true}
  | .progress => s
  | .cleanup => {held := false, cancellationRequested := s.cancellationRequested}

def workerTrace (s : Worker) : List WorkerEvent → Worker
  | [] => s
  | event :: rest => workerTrace (workerStep s event) rest

theorem cancellation_retains_credit_until_cleanup (events : List WorkerEvent) (s : Worker)
    (held : s.held = true) (notCleaned : WorkerEvent.cleanup ∉ events) :
    (workerTrace s events).held = true := by
  induction events generalizing s with
  | nil => exact held
  | cons event rest ih =>
    have tail : WorkerEvent.cleanup ∉ rest := by simp_all
    have next : (workerStep s event).held = true := by
      cases event <;> simp_all [workerStep]
    exact ih (workerStep s event) next tail

#print axioms retry_identity_cannot_move_to_new_source
#print axioms publish_keeps_binding
#print axioms arbitrary_initial_trace_has_no_holes_or_unallocated_ids
#print axioms reserve_assigns_fresh_record
#print axioms engine_write_other_id
#print axioms delayed_exact_retry_preserves_new_bytes
#print axioms reservation_preserves_allocation_bound
#print axioms step_preserves_allocation_bound
#print axioms trace_preserves_allocation_bound
#print axioms step_preserves_prefix
#print axioms arbitrary_trace_preserves_source_prefix
#print axioms existing_binding_immutable
#print axioms conflicting_retry_refused
#print axioms reservation_precedes_upsert
#print axioms unknown_retains_reservation_and_ids
#print axioms fresh_id_separates_old
#print axioms delayed_packet_preserves_fresh_id
#print axioms missing_readback_blocks_publication
#print axioms publication_requires_full_readback
#print axioms publication_never_skips_source
#print axioms step_preserves_binding
#print axioms arbitrary_trace_preserves_binding
#print axioms shadow_cannot_delete
#print axioms gc_requires_matching_confirmation
#print axioms expiry_independent_of_metadata
#print axioms unauthenticated_lease_refused
#print axioms legacy_not_retroactively_expired
#print axioms retirement_requires_expiry_and_forwarding
#print axioms pending_generation_survives_expiry
#print axioms cancellation_retains_credit_until_cleanup
end Custody
