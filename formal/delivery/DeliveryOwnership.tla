----------------------- MODULE DeliveryOwnership -----------------------
\* An event is one distinct logical identity in the finite TLC model.
CONSTANT Events
ASSUME Events # {}

\* upstream: events the sender still retains for retry
\* volatile: events accepted by the receiver but lost on receiver crash
\* durable: events whose receiver commit has survived a crash
\* acked: events for which the sender has received an acknowledgement
VARIABLES upstream, volatile, durable, acked
vars == <<upstream, volatile, durable, acked>>

Init ==
    /\ upstream = Events
    /\ volatile = {}
    /\ durable = {}
    /\ acked = {}

\* The sender keeps its copy while the receiver buffers a copy.
Receive(e) ==
    /\ e \in upstream
    /\ e \notin volatile
    /\ volatile' = volatile \cup {e}
    /\ UNCHANGED <<upstream, durable, acked>>

\* Commit abstracts a completed durable write, not a mere write call.
Commit(e) ==
    /\ e \in volatile
    /\ volatile' = volatile \ {e}
    /\ durable' = durable \cup {e}
    /\ UNCHANGED <<upstream, acked>>

\* A lost acknowledgement is represented by this action not occurring.
Acknowledge(e) ==
    /\ e \in durable
    /\ e \notin acked
    /\ acked' = acked \cup {e}
    /\ UNCHANGED <<upstream, volatile, durable>>

Forget(e) ==
    /\ e \in acked
    /\ e \in upstream
    /\ upstream' = upstream \ {e}
    /\ UNCHANGED <<volatile, durable, acked>>

ReceiverCrash ==
    /\ volatile # {}
    /\ volatile' = {}
    /\ UNCHANGED <<upstream, durable, acked>>

Next ==
    \/ \E e \in Events: Receive(e)
    \/ \E e \in Events: Commit(e)
    \/ \E e \in Events: Acknowledge(e)
    \/ \E e \in Events: Forget(e)
    \/ ReceiverCrash

Spec == Init /\ [][Next]_vars

TypeOK ==
    /\ upstream \subseteq Events
    /\ volatile \subseteq Events
    /\ durable \subseteq Events
    /\ acked \subseteq Events

\* Receiving an ACK implies a committed, crash-surviving owner.
AckSafety == acked \subseteq durable

\* Volatile memory is deliberately excluded: a crash can erase it.
RetainedOrDurable == Events \subseteq (upstream \cup durable)

\* Deliberately unsafe alternative: acknowledge a merely buffered event.
EarlyAcknowledge(e) ==
    /\ e \in volatile
    /\ e \notin acked
    /\ acked' = acked \cup {e}
    /\ UNCHANGED <<upstream, volatile, durable>>

EarlyAckNext == Next \/ \E e \in Events: EarlyAcknowledge(e)
EarlyAckSpec == Init /\ [][EarlyAckNext]_vars
=======================================================================
