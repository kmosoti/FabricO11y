# C5 preservation and readiness review

Read-only review; no native C5 service cell ran. This review changes no code,
registered protocol, cap or historical criterion. Projection/admission command
completion is distinct from performance acceptance.

The [actual projection](data/catalog-borrowed-c5-run-01/projection.json) generated
300000 offered source lines, 900-byte bodies, exactly15000 lines/node, with300
sample body-hash checks and decoded SHA readbacks for all20 gzip files. It records
111,046,844 file bytes /111,083,520 allocated bytes,12.460896 seconds, and owned
scratch removal. This measures the declared per-node gzip preservation format;
it is not a lower bound on every possible compression or regeneration scheme.

The [admission snapshot](data/catalog-borrowed-c5-run-01/admission-only.json)
records a20,971,520-byte C5 assignment including failure, versus115,277,824 bytes
reserved for measured inputs plus the4MiB successful-archive forecast.
Aggregate usage1,888,927,744 against1,894,879,232 leaves5,951,488 bytes at that
snapshot; later receipts can consume more. Capacity usage847,089,664 against
872,415,232 is a separate bound. `admitted=false` and
`native_child_started=false` are explicit.

Exact source regeneration could remove the approximately106MiB input cost after
byte-for-byte readback checks. Sharing identical frozen helpers and new seed
archives could reclaim more space. Neither establishes a bound for arbitrary
failed journals, Spools, partial Parquet/building files, control state or damaged
bytes. The current20MiB preservation plan remains unbounded for full failed
state; this is a limitation of this plan, not a global impossibility claim.

Consequential source findings:

- [Native adapter](../../../tools/bench/labs/catalog/coupled_c5_native.py),
  lines148,191,256–268: seed timeout450s, sampler900s, individual shutdown30s
  and dump180s do not compose into the proposed300s cell. Use one absolute
  deadline with cleanup reserve; require200s demand plus verification budget
  remaining before release.
- Same adapter, lines172 and221–222: TCP readiness does not establish
  authenticated HTTPS readiness; the query thread starts before sealer release.
  Validate application readiness and record a shared measurement origin plus
  actual release offset.
- [Consumer](../../../tools/bench/labs/query_compare/consumer.py), lines16–32,
  and [stop](../../../tools/bench/observe_dev_small.py), lines218–221:
 200 serial1Hz requests include drain; a slow final request can overrun shutdown.
  Retain cadence/lateness failures and include joins/API timeouts in the deadline.
  `.building` monitoring is a sampled concurrency lower bound, not utilization.
- Native adapter line346 removes replay even when summary guards failed.
  [Driver](../../../tools/bench/labs/catalog/coupled_c5.py), lines287–294,
  excludes replay and requires all four closed inputs for the normal archive.
  Early failures therefore need a separate partial-input/raw-state path; retain
  failed replay and prevent preservation errors masking the original failure.
- [Archive helper](../../../tools/bench/labs/catalog/coupled_c5_archive.py),
  line110: gzip headers vary, preventing straightforward sharing of identical
  new seed archives. Deterministic headers plus verified content addressing can
  help; they do not justify deleting unmatched failed bytes.
- Driver line118 hard-codes rejection. Replace this temporary blocker with
  executable allocation arithmetic once a tested full-state preservation bound
  exists. Compiled-cap acknowledgement alone also does not attest disabled
  unrelated experiment selectors; freeze those build settings explicitly.

Smallest fix sequence: validate/exclude reconstructible source inputs; deduplicate
new identical evidence; define and test exact failed-state preservation with
quota/readback/corruption controls; establish its bound against current aggregate
and capacity inventory; fix lifecycle deadlines/readiness/replay retention; then
register a new admission cut before any service cell. No reclaim forecast is
substituted for measured headroom or a preservation bound.

If the full fixture cannot fit, a separately preregistered small pending-journal
worker/cap correctness screen is a cheaper next experiment. It must keep enough
real files/rows to exercise concurrency and byte-driven spills, and explicitly
reduce source demand/duration and bound state. It would not complete the original
128MiB,180s shared-demand matrix or establish sustained service performance.
