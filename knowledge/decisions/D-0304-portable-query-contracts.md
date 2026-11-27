# Portable query operation contracts

A runtime-neutral `fabric-query-contract` leaf owns the existing query DTOs,
validation, decimal encodings and operation bindings. The native `fabric`
query module and `fabric-client` expose the same types; native credential and
transport work stays in `fabric`. An optional schema feature is forwarded so
existing schema-producing consumers retain the same contract.

Run identifier validation is shared with the native generator. A typed query
request and coverage metadata do not make a generic JSON response row typed;
consumers must decode and validate the row appropriate to the operation. This
boundary supplies neither authorization nor a frontend state reducer.
