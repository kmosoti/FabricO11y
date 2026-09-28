#!/usr/bin/env python3
"""Validate a delivery transcript against the DeliveryOwnership TLA+ model.

The transcript is the JSONL the delivery fault harness writes and the frozen
delivery oracle grades (see tools/qualification/DELIVERY_ORACLE.md). This tool
abstracts it into model actions and asks TLC whether *some* behavior of the
unchanged model, `DeliveryOwnership.tla`, explains it:

    attempt(e)                    -> Receive(e)
    ack committed_through = N     -> Acknowledge(e) for every sourced e <= N
                                     on that Strand not yet acknowledged
    node_state(retained)          -> Forget(e) for every sourced, not retained,
                                     not yet forgotten e on that Strand
    server kill / outage          -> may be explained by ReceiverCrash
    recovered set R (at the end)  -> durable = R
    any other record              -> no model step

Between observations the model may take hidden `Commit(e)` and
`ReceiverCrash` steps, because the transcript cannot observe the server's
volatile buffer or the moment of its durable write. Composite steps apply the
model's own actions (through `INSTANCE`), so an accepted trace is a behavior
of `Spec` projected onto the observations.

TLC searches for a behavior that reaches the end of the trace. We state that
as the invariant `TraceNotAccepted`; a violation of it means ACCEPTED. If
TLC exhausts the reachable states without violating it, no model behavior
explains the transcript: REJECTED, with the index of the furthest observation
any behavior reached.

Limits: the model has no bytes, sequences or credentials (the oracle checks
those); a node process crash is not a model action because the model assumes
the upstream copy survives, which the Spool's durability provides. This is
trace validation of one run, not a proof about the Rust code.

Usage:
    TLA_JAR=/path/tla2tools.jar python3 -B formal/delivery/trace_check.py TRANSCRIPT.jsonl
Exit status: 0 accepted, 1 rejected, 2 tool or input error.
"""

import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

MODEL_DIR = Path(__file__).resolve().parent


def ident(record):
    return f"{record['node_id']}:{record['generation']}:{record['sequence']}"


def strand(record):
    return (record["node_id"], record["generation"])


def abstract(lines):
    """Turn transcript records into (events, observations, recovered, labels)."""
    records = [json.loads(line) for line in lines if line.strip()]
    events, sourced_by_strand = [], {}
    for r in records:
        if r["type"] == "source":
            e = ident(r)
            if e not in events:
                events.append(e)
            sourced_by_strand.setdefault(strand(r), []).append(r["sequence"])
    observations, labels = [], []
    acked, forgotten = set(), set()
    seen_by_strand = {}
    recovered = set()
    for index, r in enumerate(records):
        kind = r["type"]
        if kind == "source":
            seen_by_strand.setdefault(strand(r), set()).add(r["sequence"])
        elif kind == "attempt" and not r["injected_conflict"]:
            observations.append(("receive", [ident(r)]))
            labels.append(f"record {index + 1}: attempt {ident(r)}")
        elif kind == "response" and r["kind"] == "ack":
            node, gen = strand(r)
            through = r["committed_through"]
            ids = [f"{node}:{gen}:{s}" for s in sourced_by_strand.get(strand(r), [])
                   if s <= through and f"{node}:{gen}:{s}" not in acked]
            acked.update(ids)
            observations.append(("ack", ids))
            labels.append(f"record {index + 1}: ack {node}:{gen} through {through}")
        elif kind == "node_state":
            node, gen = r["node_id"], r["generation"]
            retained = set(r["retained_sequences"])
            ids = [f"{node}:{gen}:{s}" for s in sorted(seen_by_strand.get((node, gen), set()))
                   if s not in retained and f"{node}:{gen}:{s}" not in forgotten]
            # A retained sequence must still be upstream: forgetting is final.
            back = [f"{node}:{gen}:{s}" for s in retained if f"{node}:{gen}:{s}" in forgotten]
            forgotten.update(ids)
            observations.append(("forget", ids, back))
            labels.append(f"record {index + 1}: node_state {node}:{gen} retains {sorted(retained)}")
        elif kind == "recovered":
            recovered.add(ident(r))
    observations.append(("end", sorted(recovered)))
    labels.append("end: durable set equals the recovered set")
    return events, observations, recovered, labels


def tla_set(items):
    return "{" + ", ".join(json.dumps(i) for i in items) + "}"


def tla_trace(observations):
    rows = []
    for obs in observations:
        if obs[0] == "forget":
            rows.append(f'[k |-> "forget", s |-> {tla_set(obs[1])}, back |-> {tla_set(obs[2])}]')
        else:
            rows.append(f'[k |-> "{obs[0]}", s |-> {tla_set(obs[1])}, back |-> {{}}]')
    return "<<\n    " + ",\n    ".join(rows) + "\n>>"


MODULE = """---- MODULE DeliveryTrace ----
EXTENDS Naturals, Sequences
VARIABLES upstream, volatile, durable, acked, i
TraceEvents == {events}
D == INSTANCE DeliveryOwnership WITH Events <- TraceEvents
Trace == {trace}

Init == D!Init /\\ i = 1

\\* Unobservable steps: the server's commit and a receiver crash.
Hidden ==
    /\\ \\/ \\E e \\in TraceEvents: D!Commit(e)
       \\/ D!ReceiverCrash
    /\\ UNCHANGED i

\\* A set of Acknowledge (or Forget) actions, each enabled in turn.
AckAll(S) ==
    /\\ S \\subseteq durable
    /\\ S \\cap acked = {{}}
    /\\ acked' = acked \\cup S
    /\\ UNCHANGED <<upstream, volatile, durable>>

ForgetAll(S) ==
    /\\ S \\subseteq acked
    /\\ S \\subseteq upstream
    /\\ upstream' = upstream \\ S
    /\\ UNCHANGED <<volatile, durable, acked>>

Observe ==
    /\\ i <= Len(Trace)
    /\\ LET t == Trace[i] IN
        CASE t.k = "receive" -> \\E e \\in t.s: D!Receive(e)
          [] t.k = "ack"     -> AckAll(t.s)
          [] t.k = "forget"  -> t.back = {{}} /\\ ForgetAll(t.s)
          [] t.k = "end"     -> durable = t.s /\\ UNCHANGED <<upstream, volatile, durable, acked>>
    /\\ i' = i + 1

Next == Observe \\/ Hidden

TraceNotAccepted == i <= Len(Trace)
\\* The model's own safety invariants hold along every explaining behavior.
Safety == D!AckSafety /\\ D!RetainedOrDurable
====
"""

CONFIG = """INIT Init
NEXT Next
INVARIANT Safety
INVARIANT TraceNotAccepted
"""


def run_tlc(events, observations, jar, java="java"):
    work = Path(tempfile.mkdtemp(prefix="delivery-trace-"))
    try:
        shutil.copy(MODEL_DIR / "DeliveryOwnership.tla", work)
        (work / "DeliveryTrace.tla").write_text(
            MODULE.format(events=tla_set(events), trace=tla_trace(observations)))
        (work / "DeliveryTrace.cfg").write_text(CONFIG)
        proc = subprocess.run(
            [java, "-XX:+UseParallelGC", "-cp", jar, "tlc2.TLC", "-deadlock", "-workers", "1",
             "-metadir", str(work / "states"), "-config", "DeliveryTrace.cfg", "DeliveryTrace.tla"],
            cwd=work, capture_output=True, text=True, timeout=1800)
        return proc.stdout + proc.stderr
    finally:
        shutil.rmtree(work, ignore_errors=True)


def verdict(output, labels):
    if "Invariant TraceNotAccepted is violated" in output:
        return "accepted", None
    if "Invariant Safety is violated" in output:
        return "unsafe", None
    if "Model checking completed. No error has been found." in output:
        return "rejected", None
    return "error", None


def check(lines, jar):
    events, observations, _, labels = abstract(lines)
    output = run_tlc(events, observations, jar)
    result, _ = verdict(output, labels)
    return result, output, labels


def main(argv):
    if len(argv) != 2:
        print(__doc__.split("Usage:")[1].strip(), file=sys.stderr)
        return 2
    jar = os.environ.get("TLA_JAR")
    if not jar or not Path(jar).is_file():
        print("TLA_JAR must name tla2tools.jar", file=sys.stderr)
        return 2
    lines = Path(argv[1]).read_text().splitlines()
    result, output, labels = check(lines, jar)
    events, observations, recovered, _ = abstract(lines)
    print(json.dumps({"result": result, "events": len(events),
                      "observations": len(observations), "recovered": len(recovered)}))
    if result == "accepted":
        return 0
    if result in ("rejected", "unsafe"):
        print("no DeliveryOwnership behavior explains this transcript", file=sys.stderr)
        return 1
    print(output[-3000:], file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))
