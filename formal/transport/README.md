# Transport ownership and credit model

This finite TLA+ model checks the stated transport contract for two producers,
each with one `(producer, sequence)` identity. Each message is two bytes, the
receiver byte cap is two bytes, and the global credit plus scheduled-packet
budget is two packets. An identity stays stable on retry.

`Safe.cfg` exhaustively checks these safety properties:

- A sender retains its copy until a durable ACK is delivered. CREDIT only
  permits a scheduled send; a send consumes one previously granted credit.
- Outstanding credits plus scheduled packets in flight never exceed the packet
  budget.
- Receiver admission stays within the byte cap. An over-cap packet takes an
  explicit `Reject` transition.
- ACK creation and delivery follow commit. Each identity has at most one
  commit. A retry after a lost ACK gets a deduplication response and cannot
  create a second commit.

`EarlyAck.cfg` enables a deliberately broken action. TLC must find an
`AckSafety` counterexample after `EarlyAcknowledge`. `OverGrant.cfg` enables a
separate broken action. Its `CreditBound` counterexample has two outstanding
credits and one scheduled packet against a budget of two. `RetryWitness.cfg`
checks reachability of the normal `LoseAck → SendRetry → RespondDuplicate`
path by asking TLC to refute `NoDedupResponse`. Its expected exit 12 is a
witness, not a safety failure in `Safe.cfg`.

Run with the same cached TLC jar and optional Java executable pattern as
`formal/delivery/check.sh`:

```sh
TLA_JAR=/absolute/path/to/tla2tools.jar \
  JAVA_BIN=/absolute/path/to/java ./formal/transport/check.sh
```

The model tracks one whole packet per identity and permits one retry after a
lost durable ACK to keep the state graph finite. It checks safety and path
reachability, not eventual delivery under arbitrary stuttering or repeated
loss. The receiver byte count is retained for admitted identities throughout
this bounded run.

## Open questions

- What grants or replenishes CREDIT, and is credit shared globally or assigned
  to a producer or connection? Does unused credit expire or get revoked?
- Does the packet budget include control packets or only scheduled data? How
  are fragmented messages counted against the packet and byte bounds?
- When are receiver bytes released, and do committed or ACKed messages still
  consume this admission cap?
- What is the sender's required behavior after explicit rejection: stop,
  retry after space becomes available, or report a terminal error? Does a
  rejection return credit or consume it permanently?
- What retry timing, fairness, and maximum attempts are required after ACK
  loss, including repeated ACK loss? May duplicates be sent before the first
  commit, or may several copies of one identity be in flight concurrently?
- Are data packets or CREDIT messages also lossy? What recovery is required
  across sender or receiver crashes, and what durable storage defines commit?
- Is ACK per identity or cumulative, and can ACKs be reordered or coalesced?

These choices are not fixed by the stated contract. The bounded model makes
only the assumptions needed to explore the specified safety obligations.
