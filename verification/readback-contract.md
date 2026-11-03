# Acknowledged observation lifecycle

Every acknowledged observation must remain retrievable through bounded read and
export interfaces, including after restart. The accepted wire domain includes
all valid JSON spellings, not only output from the SDK serializer. Bounds must
account for response envelopes and representation changes. Existing stored records
retain the same bytes and retry identities; no database migration is required.

Independent fixtures cover valid depth boundaries, escaped strings, compact numeric
spellings and combined size/depth cases. Submit, acknowledgement, read, restart,
read and export must succeed, and export must progress to a following record.
Negative controls restore the original depth and normalization defects and must
fail the corresponding regression. Numeric analysis checks include defined
positive rescaling and finite magnitude extremes.

Required lifecycle obligations must identify boundaries, named executable checks,
proof scope and assumptions. Missing, stale, unmatched, ignored or zero-execution
checks cannot establish coverage. A passing command and coverage are distinct.
Evidence binds source and manifest identity. Coverage represents tested declared
obligations; it does not establish completeness of the declarations themselves.
