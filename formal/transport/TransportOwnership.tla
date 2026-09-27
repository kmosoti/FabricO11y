------------------------ MODULE TransportOwnership ------------------------
EXTENDS Naturals, FiniteSets

CONSTANTS Producers, MessageBytes, ReceiverByteCap, CreditBudget

ASSUME Cardinality(Producers) = 2
ASSUME MessageBytes \in Nat \ {0}
ASSUME ReceiverByteCap \in Nat
ASSUME CreditBudget \in Nat \ {0}

Messages == {<<p, 1>> : p \in Producers}

VARIABLES credits, inFlight, retained, admitted, rejected,
          committed, commitCount, ackPending, acked,
          lostAck, sendCount, dedupResponded

vars == <<credits, inFlight, retained, admitted, rejected,
          committed, commitCount, ackPending, acked,
          lostAck, sendCount, dedupResponded>>

UsedBytes == MessageBytes * Cardinality(admitted)

Init ==
    /\ credits = 0
    /\ inFlight = {}
    /\ retained = Messages
    /\ admitted = {}
    /\ rejected = {}
    /\ committed = {}
    /\ commitCount = [m \in Messages |-> 0]
    /\ ackPending = {}
    /\ acked = {}
    /\ lostAck = {}
    /\ sendCount = [m \in Messages |-> 0]
    /\ dedupResponded = {}

\* A credit is permission for one scheduled packet. The receiver may only
\* grant slots that fit the global credit-plus-in-flight packet budget.
GrantCredit ==
    /\ credits + Cardinality(inFlight) < CreditBudget
    /\ credits' = credits + 1
    /\ UNCHANGED <<inFlight, retained, admitted, rejected,
                   committed, commitCount, ackPending, acked,
                   lostAck, sendCount, dedupResponded>>

SendFresh(m) ==
    /\ m \in Messages
    /\ m \in retained
    /\ sendCount[m] = 0
    /\ credits > 0
    /\ credits' = credits - 1
    /\ inFlight' = inFlight \cup {m}
    /\ sendCount' = [sendCount EXCEPT ![m] = 1]
    /\ UNCHANGED <<retained, admitted, rejected, committed,
                   commitCount, ackPending, acked, lostAck,
                   dedupResponded>>

\* A lost durable ACK licenses one bounded retry in this finite model.
\* The same m is reused: retry has no way to allocate a new identity.
SendRetry(m) ==
    /\ m \in Messages
    /\ m \in retained
    /\ m \in lostAck
    /\ sendCount[m] = 1
    /\ credits > 0
    /\ credits' = credits - 1
    /\ inFlight' = inFlight \cup {m}
    /\ sendCount' = [sendCount EXCEPT ![m] = 2]
    /\ UNCHANGED <<retained, admitted, rejected, committed,
                   commitCount, ackPending, acked, lostAck,
                   dedupResponded>>

Admit(m) ==
    /\ m \in inFlight
    /\ m \notin committed
    /\ m \notin admitted
    /\ UsedBytes + MessageBytes <= ReceiverByteCap
    /\ inFlight' = inFlight \ {m}
    /\ admitted' = admitted \cup {m}
    /\ UNCHANGED <<credits, retained, rejected, committed,
                   commitCount, ackPending, acked, lostAck,
                   sendCount, dedupResponded>>

Reject(m) ==
    /\ m \in inFlight
    /\ m \notin committed
    /\ m \notin admitted
    /\ UsedBytes + MessageBytes > ReceiverByteCap
    /\ inFlight' = inFlight \ {m}
    /\ rejected' = rejected \cup {m}
    /\ UNCHANGED <<credits, retained, admitted, committed,
                   commitCount, ackPending, acked, lostAck,
                   sendCount, dedupResponded>>

\* Durable commit precedes creation of its ACK.
Commit(m) ==
    /\ m \in admitted
    /\ m \notin committed
    /\ committed' = committed \cup {m}
    /\ commitCount' = [commitCount EXCEPT ![m] = @ + 1]
    /\ ackPending' = ackPending \cup {m}
    /\ UNCHANGED <<credits, inFlight, retained, admitted, rejected,
                   acked, lostAck, sendCount, dedupResponded>>

\* A retried committed identity produces another ACK, not another commit.
RespondDuplicate(m) ==
    /\ m \in inFlight
    /\ m \in committed
    /\ inFlight' = inFlight \ {m}
    /\ ackPending' = ackPending \cup {m}
    /\ dedupResponded' = dedupResponded \cup {m}
    /\ UNCHANGED <<credits, retained, admitted, rejected,
                   committed, commitCount, acked, lostAck, sendCount>>

LoseAck(m) ==
    /\ m \in ackPending
    /\ m \notin acked
    /\ ackPending' = ackPending \ {m}
    /\ lostAck' = lostAck \cup {m}
    /\ UNCHANGED <<credits, inFlight, retained, admitted, rejected,
                   committed, commitCount, acked, sendCount,
                   dedupResponded>>

DeliverAck(m) ==
    /\ m \in ackPending
    /\ m \in committed
    /\ ackPending' = ackPending \ {m}
    /\ acked' = acked \cup {m}
    /\ retained' = retained \ {m}
    /\ UNCHANGED <<credits, inFlight, admitted, rejected, committed,
                   commitCount, lostAck, sendCount, dedupResponded>>

Next ==
    \/ GrantCredit
    \/ \E m \in Messages:
         SendFresh(m) \/ SendRetry(m) \/ Admit(m) \/ Reject(m)
         \/ Commit(m) \/ RespondDuplicate(m) \/ LoseAck(m)
         \/ DeliverAck(m)

SafeSpec == Init /\ [][Next]_vars

\* This action deliberately violates the durable-ACK order. It is enabled
\* only by BrokenSpec, never by SafeSpec.
EarlyAcknowledge(m) ==
    /\ m \in admitted
    /\ m \notin committed
    /\ ackPending' = ackPending \cup {m}
    /\ UNCHANGED <<credits, inFlight, retained, admitted, rejected,
                   committed, commitCount, acked, lostAck,
                   sendCount, dedupResponded>>

BrokenNext == Next \/ \E m \in Messages: EarlyAcknowledge(m)
BrokenSpec == Init /\ [][BrokenNext]_vars

\* Deliberate credit-rule defect: grant a third slot while a scheduled
\* packet is in flight and the full two-slot budget is already reserved.
OverGrantCredit ==
    /\ inFlight # {}
    /\ credits + Cardinality(inFlight) = CreditBudget
    /\ credits' = credits + 1
    /\ UNCHANGED <<inFlight, retained, admitted, rejected,
                   committed, commitCount, ackPending, acked,
                   lostAck, sendCount, dedupResponded>>

OverGrantNext == Next \/ OverGrantCredit
OverGrantSpec == Init /\ [][OverGrantNext]_vars

TypeOK ==
    /\ credits \in 0..CreditBudget
    /\ inFlight \subseteq Messages
    /\ retained \subseteq Messages
    /\ admitted \subseteq Messages
    /\ rejected \subseteq Messages
    /\ committed \subseteq Messages
    /\ ackPending \subseteq Messages
    /\ acked \subseteq Messages
    /\ lostAck \subseteq Messages
    /\ dedupResponded \subseteq Messages
    /\ commitCount \in [Messages -> 0..1]
    /\ sendCount \in [Messages -> 0..2]

CreditBound == credits + Cardinality(inFlight) <= CreditBudget
AdmissionBound == UsedBytes <= ReceiverByteCap
RejectedExplicitly == rejected \cap admitted = {}
RetainUntilAck == retained = Messages \ acked
AckSafety == ackPending \subseteq committed /\ acked \subseteq committed
CommitOnce == \A m \in Messages: commitCount[m] <= 1
CommitRecorded == committed = {m \in Messages : commitCount[m] = 1}
DedupSafety == dedupResponded \subseteq committed
\* Negating this observer lets TLC produce a retry/dedup witness trace.
NoDedupResponse == dedupResponded = {}

=============================================================================
