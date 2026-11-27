# Explicit corpus as-of views

A bounded owned view freezes selected revision and relation digests, including
selections that are absent from the source corpus. It also freezes attributed
conjunctive support groups. Each group names a conclusion and an explicit map
from premise digests to exact relation digests. Resolution requires the
conclusion, every premise, and each selected relation to exist in the view;
each relation must assert `Supports` from that premise to that conclusion.
Missing and mismatched inputs remain explicit, and selected dissent records and
relations remain available. No truth, entailment, custody or time ordering is
inferred from these recorded claims.

The view caps selected revisions, relations, groups, premises and total
canonical record bytes before copying bodies. Ordered collections give
repeatable results for the same selected inputs. Adapters must bound decoding
and reject duplicates before constructing sets and maps. A view is an in-memory
projection, not an authorization check, durable cutoff, active-head algorithm,
serialized format, availability guarantee, or retention policy. Later corpus
admission cannot change an already captured view.
