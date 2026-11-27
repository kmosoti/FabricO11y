# Conditional canonical plan acceptance

A decision-writer credential binds an operator-provisioned plan scope to one
owner and allowed proposal origin/producer. Reader and node credentials do not
grant this authority. The canonical SQLite transaction checks the owner and
recovers an identical scoped command before comparing its expected head, then
stores the immutable decision, head and exact result together. Proposal bytes
are checked against the locally committed observation identity and hash;
projection visibility is a later fact.

The first profile has static scope bindings and bounded non-evicting decision
history. A full history refuses new decisions while retaining retry results.
Durable relay identity floors, policy transfer, review profiles, and external
execution remain separate contracts.
