# Canonical query operation negotiation

An additive canonical command carries the existing query request and a separate
unsigned 32-bit operation version. The authenticated receiver requires its
reader role, then matches that version against the actual query variant and
its top-level protocol version before query preparation or analytical admission.
The checked request continues through the existing query path. The query body,
legacy command, response and authorization policies remain unchanged.

`fabric-query query-versioned VERSION` opts into the new command with the
existing stdin request and credential configuration. Unsupported versions are
rejected, with no discovery roundtrip or automatic downgrade. Older receivers
may reject the new command; callers choose the legacy command explicitly when
appropriate. This contract does not negotiate plan, Spindle, MCP or every
future operation family, and it does not grant execution authority beyond the
existing authenticated query role.
