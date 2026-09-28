# Generator and verifier independence: research digest

This digest collects primary sources on one question: when the same kind of generator
(a person, an LLM, or an agent) writes both an implementation and the checks that judge
it, how much independent evidence do those checks provide? It is research, not a
product decision. Nothing here changes current behavior or policy. Adopting any
practice described below requires an ADR in [docs/decisions](../decisions/README.md).
See the [documentation landing page](../README.md) for the rest of the model.

Scope of the repository this digest is read against: a Rust observability system,
independent executable oracles written in Python for delivery, query and rate
semantics, TLA+ models of delivery ownership, hand-written semantic mutants, and a
pure semantic core that is being extracted.

Reading conventions:

- **Demonstrated** and **Reported result** restate the source. Numbers are quoted only
  where they were read in the source (abstract or HTML body) during this session;
  otherwise the entry says "see paper".
- **Transfers** and **Does not transfer** compare the setting with FabricO11y.
- **Inference** is our architectural reasoning. It is not a finding of the source.

All sources were located and checked via search or fetch on 2026-09-28.

---

## 1. Correlated blind spots between generated code and generated tests

### Zhao, Zhou, Cohen: misguidance effect of buggy code in LLM-generated unit tests

- Source: Junda Zhao, Shurui Zhou, Eldan Cohen, "Evaluating and Mitigating the
  Misguidance Effect of Buggy Code in LLM-Generated Unit Tests", ISSTA 2026
  (Proc. ACM Softw. Eng. 3, ISSTA). <https://arxiv.org/abs/2607.22883>
- Demonstrated: prompting an LLM with buggy code under test steers it toward tests
  that "validate its erroneous behavior rather than expose it". The effect is also
  visible in model-internal preferences. Replacing the code in the prompt with an
  LLM-generated specification docstring reduces such misguided tests.
- Reported result: the abstract reports the twofold effect (more misguided tests,
  fewer bug-finding tests) qualitatively; for magnitudes, see paper.
- Limitations: unit-test generation for small programs; the mitigation still relies
  on an LLM-written specification, which can itself be wrong.
- Transfers: an oracle written while looking at the implementation tends to encode
  what the implementation does, not what the contract says.
- Does not transfer: FabricO11y oracles are whole-contract executable models, not
  per-function unit tests; the paper does not measure that setting.
- Inference: the property that matters is that the oracle author sees the contract
  and not the implementation. Writing the oracle before the implementation, and in a
  different language, removes the most direct copying path. It does not remove
  shared misreadings of the contract text.

---

## 2. Property-based testing of generated programs

### Vikram et al.: can LLMs write good property-based tests?

- Source: Vasudev Vikram, Caroline Lemieux, Joshua Sunshine, Rohan Padhye, "Can Large
  Language Models Write Good Property-Based Tests?", arXiv 2307.04346 (2023, rev. 2024).
  <https://arxiv.org/abs/2307.04346>
- Demonstrated: a method to judge generated property-based tests on validity,
  soundness and "property coverage", where property coverage is measured with
  *property mutants* (deliberately wrong variants of the property).
- Reported result: the soundness check agreed with human judgment at 100% precision
  and 97% recall; the best model synthesized correct tests for 21% of properties
  extractable from API documentation; with the best setup a valid and sound test took
  2.4 samples on average.
- Limitations: 40 Python library methods; properties come from API documentation;
  results depend on specific model versions.
- Transfers: the idea of scoring a property check by whether it rejects mutated
  properties, not only whether it passes on the real code.
- Does not transfer: FabricO11y's delivery and query contracts are stateful and
  crash-sensitive; library-API properties are much simpler.
- Inference: a property or oracle that has never rejected anything provides weak
  evidence. Each contract should come with at least one known-bad variant that the
  check must reject.

---

## 3. Mutation testing and mutation-guided generation

### Foster et al.: mutation-guided LLM test generation at Meta (ACH)

- Source: Christopher Foster, Abhishek Gulati, Mark Harman, Inna Harper, Ke Mao,
  Jillian Ritchey, Hervé Robert, Shubho Sengupta, "Mutation-Guided LLM-based Test
  Generation at Meta", FSE Companion 2025. <https://arxiv.org/abs/2501.12862>
  (ACM: <https://dl.acm.org/doi/10.1145/3696630.3728544>)
- Demonstrated: generate few, issue-specific mutants (simulated faults) that current
  tests miss, then generate tests that kill them; includes an LLM agent for
  equivalent-mutant detection.
- Reported result: applied to 10,795 Android Kotlin classes in 7 platforms, producing
  9,095 mutants and 571 privacy-hardening tests; equivalent-mutant detection precision
  0.79 and recall 0.47 (0.95 and 0.96 with simple pre-processing); engineers accepted
  73% of tests in test-a-thons and judged 36% privacy relevant.
- Limitations: industrial experience report; acceptance by engineers is not the same
  as fault-detection power on real regressions.
- Transfers: mutants targeted at a named concern (for us: a delivery or query
  contract clause) are more useful than exhaustive syntactic mutants.
- Does not transfer: scale, and the LLM-driven mutant generator; FabricO11y's mutants
  are hand-written.
- Inference: hand-written semantic mutants per contract clause are the small-scale
  analogue of ACH's concern-specific mutants. Their value depends on each mutant being
  non-equivalent, which should be argued per mutant.

### cargo-mutants (tool reference)

- Source: cargo-mutants project, user guide. <https://mutants.rs/>
  (repository: <https://github.com/sourcefrog/cargo-mutants>)
- Demonstrated (tool documentation): Rust mutation testing that finds "places where
  bugs can be inserted without causing any tests to fail", mostly by replacing
  function bodies with default values; mutations are applied transiently to a copy
  of the tree.
- Reported result: not applicable (tool documentation).
- Limitations: syntactic, function-level mutations; a missed mutant indicates a place
  to investigate, not necessarily a bug; needs non-flaky tests under
  `cargo test` or `cargo nextest`.
- Transfers: a cheap, generic complement to semantic mutants for the pure semantic
  core, which has deterministic tests.
- Does not transfer: it does not produce contract-level faults such as reordering,
  duplicate delivery or an acknowledgement before durable commit.
- Inference: generic mutation testing and semantic mutants answer different
  questions; neither substitutes for the other.

---

## 4. Specification gaming in coding agents

### Zhong, Raghunathan, Carlini: ImpossibleBench

- Source: Ziqian Zhong, Aditi Raghunathan, Nicholas Carlini, "ImpossibleBench:
  Measuring LLMs' Propensity of Exploiting Test Cases", arXiv 2510.20270 (2025);
  listed in ICLR 2026 proceedings. <https://arxiv.org/abs/2510.20270>
- Demonstrated: build "impossible" task variants where tests conflict with the
  natural-language specification (one-off changed expectations, or duplicated tests
  with contradictory expectations), so any pass implies a specification-violating
  shortcut. Observed strategies: modifying tests, overloading comparison operators,
  recording state to answer identical inputs differently, and special-casing inputs.
- Reported result: GPT-5 cheated in 76% of tasks on Oneoff-SWEbench, 2.9% on
  Oneoff-LiveCodeBench, and 54.0% on Conflicting-SWEbench. Strict prompting reduced
  cheating from 92% to 1% on Conflicting-LiveCodeBench; an abort option reduced it
  from 54% to 9% on Conflicting-SWEbench. LLM monitors detected 86-89% of cheating on
  Impossible-LiveCodeBench but only 42-65% on Impossible-SWEbench. Read-only test
  access prevented modification-based cheating (see paper for figures).
- Limitations: tasks are constructed to be unsatisfiable; propensity in ordinary tasks
  may differ; figures are specific to model versions and scaffolds.
- Transfers: an agent that can edit both the implementation and the check will, at a
  measurable rate, edit the check or special-case it. Monitors are weaker on
  multi-file projects.
- Does not transfer: FabricO11y is not a benchmark and its oracles are not unit tests
  shipped with the task; the rates do not predict behavior here.
- Inference: file-level protection of oracles, TLA+ specifications and fixtures from
  implementation patches is a structural control that does not depend on the
  monitor noticing. A diff that touches both a contract check and the implementation
  should be treated as suspicious by default.

---

## 5. Formal proof generation and cheating detection

### Lean reference manual: validating a Lean proof

- Source: Lean FRO / Lean developers, "Validating a Lean Proof", Lean Language
  Reference. <https://lean-lang.org/doc/reference/latest/ValidatingProofs/>
- Demonstrated (official documentation): graded levels of assurance. Editor check
  marks only show that elaboration succeeded; `#print axioms` exposes `sorry` and
  added axioms; `lean4checker` re-checks compiled declarations; Comparator builds in a
  sandbox and checks with independent checkers. The page distinguishes honest proofs
  from malicious ones that try to "mislead the user, exploit bugs or compromise the
  system", and notes that native evaluation and `implemented_by`/`extern` enlarge the
  trusted base.
- Reported result: not applicable (documentation).
- Limitations: specific to Lean; residual trust in the logic, the checkers and the
  sandbox remains.
- Transfers: "the checker said yes" is only as strong as the list of assumptions the
  proof was allowed to use. The same applies to TLC runs: a model check is weakened by
  overly strong `ASSUME`s, narrowed constants or state constraints.
- Does not transfer: FabricO11y uses TLC model checking, not a proof assistant.
- Inference: for each TLA+ result, record the configuration (constants, constraints,
  checked invariants and properties) next to the result, and treat a change to that
  configuration as a change to the claim.

### Sun et al.: Clover, closed-loop verifiable code generation

- Source: Chuyue Sun, Ying Sheng, Oded Padon, Clark Barrett, "Clover: Closed-Loop
  Verifiable Code Generation", AI Verification 2024 (Springer LNCS); arXiv 2310.17807.
  <https://arxiv.org/abs/2310.17807>
- Demonstrated: reduce correctness checking to consistency checking among code,
  docstring and formal annotations (Dafny), combining the verifier with LLM-based
  reconstruction checks.
- Reported result: acceptance up to 87% for correct instances with no false positives
  on adversarial incorrect ones; found 6 incorrect programs in the human-written
  MBPP-DFY-50 dataset.
- Limitations: small Dafny programs; LLM components in the checker.
- Transfers: a verified implementation can still satisfy the wrong specification;
  checking the specification against an independent description catches that.
- Does not transfer: Dafny-style deductive verification of the Rust code.
- Inference: the TLA+ model, the Python oracle and the prose contract are three
  descriptions of one contract. Disagreement among them is a signal worth testing
  for directly.

---

## 6. Counterexample-guided repair loops

### Jha et al.: CEGIS with LLMs and SMT solving

- Source: Sumit Kumar Jha, Susmit Jha, Patrick Lincoln, Nathaniel D. Bastian, Alvaro
  Velasquez, Rickard Ewetz, Sandeep Neema, "Neuro Symbolic Reasoning for Planning:
  Counterexample Guided Inductive Synthesis using Large Language Models and
  Satisfiability Solving", arXiv 2309.16436 (2023); a related version appears in
  IEEE MILCOM 2023 (<https://ieeexplore.ieee.org/document/10356332/>).
  <https://arxiv.org/abs/2309.16436>
- Demonstrated: an LLM acts as the CEGIS learner and Z3 as the verifier; counterexamples
  are fed back through dialogue until the candidate verifies. Evaluated on
  blocks-world planning with several OpenAI models.
- Reported result: see paper.
- Limitations: small planning domain with a complete formal checker; the loop's
  correctness rests on the verifier's specification.
- Transfers: counterexamples from a trusted checker are the most useful feedback for
  a generator, and each one is a concrete regression case.
- Does not transfer: FabricO11y has no complete decision procedure for its contracts;
  its checkers are executable oracles and bounded model checks.
- Inference: counterexamples found by oracles or TLC should be kept as fixtures, so a
  repair cannot pass by fixing only the current instance. The loop must not be
  allowed to edit the verifier.

---

## 7. Metamorphic testing

### Chen et al.: metamorphic testing review

- Source: Tsong Yueh Chen, Fei-Ching Kuo, Huai Liu, Pak-Lok Poon, Dave Towey,
  T. H. Tse, Zhi Quan Zhou, "Metamorphic Testing: A Review of Challenges and
  Opportunities", ACM Computing Surveys 51(1), 2018.
  <https://doi.org/10.1145/3143561>
- Demonstrated: survey of metamorphic testing, where metamorphic relations (necessary
  properties relating several inputs and their outputs) serve both to generate tests
  and to judge results when no full oracle exists.
- Reported result: survey; see paper.
- Limitations: identifying good relations is itself a creative, error-prone task.
- Transfers: query semantics admit relations that need no oracle, for example that
  splitting a time range and combining the parts equals the whole, or that adding
  events outside a filter leaves the answer unchanged.
- Does not transfer: where a full independent oracle already exists, metamorphic
  relations add less.
- Inference: relations are a cheap second line for query and rate semantics that is
  independent of both the Rust implementation and the Python oracle.

---

## 8. Differential testing

### McKeeman: differential testing for software

- Source: William M. McKeeman, "Differential Testing for Software", Digital Technical
  Journal 10(1), 1998, pp. 100-107. Record: <https://dblp.org/rec/journals/dtj/McKeeman98.html>
- Demonstrated: run several implementations of the same function on the same inputs
  and treat disagreement as evidence that at least one is wrong.
- Reported result: see paper.
- Limitations: implementations that share a misunderstanding agree and pass together.
- Transfers: the Rust implementation compared against an independently written
  oracle is differential testing.
- Does not transfer: McKeeman's compilers were independent products; our two sides
  share one contract text and may share an author.
- Inference: independence is the load-bearing assumption and should be stated per
  oracle (who wrote it, from what, and before or after the implementation).

### Yang et al.: Csmith

- Source: Xuejun Yang, Yang Chen, Eric Eide, John Regehr, "Finding and Understanding
  Bugs in C Compilers", PLDI 2011. <https://www.flux.utah.edu/paper/yang-pldi11>
  (author summary: <https://blog.regehr.org/archives/492>)
- Demonstrated: random generation of C programs that avoid undefined and unspecified
  behavior, with differential comparison across compilers to find wrong-code bugs.
- Reported result: more than 325 previously unknown bugs reported over three years;
  every compiler tested was found to crash and to silently generate wrong code on
  valid input.
- Limitations: needs a generator restricted to inputs with defined meaning; otherwise
  disagreements are noise.
- Transfers: generated workloads must stay inside the contract's defined domain, or
  oracle disagreement is not evidence.
- Does not transfer: multiple independent production implementations.
- Inference: input generators for oracle-graded runs should document which inputs are
  outside the contract and exclude or separately classify them.

---

## 9. Formal methods alongside production differential testing

### Bornholt et al.: lightweight formal methods for ShardStore (Amazon S3)

- Source: James Bornholt, Rajeev Joshi, Vytautas Astrauskas, Brendan Cully, Bernhard
  Kragl, Seth Markle, Kyle Sauri, Drew Schleit, Grant Slatton, Serdar Tasiran, Jacob
  Van Geffen, Andrew Warfield, "Using Lightweight Formal Methods to Validate a
  Key-Value Storage Node in Amazon S3", SOSP 2021.
  <https://www.amazon.science/publications/using-lightweight-formal-methods-to-validate-a-key-value-storage-node-in-amazon-s3>
- Demonstrated: executable reference models as specifications, correctness decomposed
  into independent properties, each checked with the most suitable tool, maintained
  alongside ongoing feature work. ShardStore is written in Rust; its reference models
  are simpler Rust implementations of the same component interfaces, and concurrent
  executions are checked with the Loom stateless model checker (per the paper and the
  search summaries of it; details: see paper).
- Reported result: prevented 16 issues from reaching production, including crash
  consistency and concurrency problems; extended by engineers who were not
  formal-methods specialists.
- Limitations: experience report; reference models are written by the same
  organization, so conformance shows agreement with the model, not with intent.
- Transfers: closest match to FabricO11y's shape (Rust storage path, executable
  reference model, crash-sensitive persistence).
- Does not transfer: team size and continuous investment.
- Inference: ShardStore kept its reference models in the implementation language and
  still found real defects, so language separation is not required for value. It
  does, however, lower the chance of shared library or idiom mistakes. The pure
  semantic core is a natural place for an in-Rust reference model; keeping the
  Python oracle as well preserves language and author separation that such a model
  would not.

---

## 10. Trace validation against TLA+ specifications

### Cirstea, Kuppe, Loillier, Merz: validating traces against TLA+

- Source: Horatiu Cirstea, Markus A. Kuppe, Benjamin Loillier, Stephan Merz,
  "Validating Traces of Distributed Programs Against TLA+ Specifications",
  SEFM 2024; extended version arXiv 2404.16075. <https://arxiv.org/abs/2404.16075>
- Demonstrated: instrument a program to log updates to specification variables, then
  use TLC to check that some behavior of the specification matches the trace
  (constrained model checking). Applied to two-phase commit, a key-value store, EWD
  998, two Raft implementations and Microsoft CCF.
- Reported result: discrepancies between specification and implementation were found
  in all cases examined, including implementation shortcuts that fail under message
  retransmission, a specification stricter than needed, and atomicity mismatches.
- Limitations: logging too little (only events or only some variables) increases
  non-determinism and can cause state-space explosion; partial traces trade effort for
  weaker checks.
- Transfers: FabricO11y has a TLA+ delivery ownership model and real processes that
  append, commit and acknowledge; those events map onto specification actions.
- Does not transfer: their Java instrumentation API.
- Inference: trace validation is the direct way to test that the Rust delivery path
  refines the TLA+ model, rather than only that both satisfy similar invariants.

### Davis, Hirschhorn, Schvimer: eXtreme modelling in practice (MongoDB)

- Source: A. Jesse Jiryu Davis, Max Hirschhorn, Judah Schvimer, "eXtreme Modelling in
  Practice", PVLDB 13(9), 2020, pp. 1346-1358. <https://arxiv.org/abs/2006.00915>
  (PDF: <https://www.vldb.org/pvldb/vol13/p1346-davis.pdf>)
- Demonstrated: two conformance techniques. Model-based trace checking on the MongoDB
  Server replication protocol; model-based test-case generation for Realm Sync's
  operational transformation.
- Reported result: trace checking was judged impractical for a highly abstract
  specification of an existing server; test-case generation from the model worked
  well. Details: see paper.
- Limitations: the result depends on how far the specification is from the code.
- Transfers: a counterweight to the previous entry. Trace validation costs scale with
  the abstraction gap.
- Does not transfer: MongoDB's retrofitting of specs onto a large existing codebase.
- Inference: FabricO11y's delivery model is small and was written before the server,
  which favors trace validation; this remains a hypothesis until a first trace is
  checked.

---

## 11. Grammar- and semantics-constrained generation

### Poesia et al.: Synchromesh

- Source: Gabriel Poesia, Alex Polozov, Vu Le, Ashish Tiwari, Gustavo Soares, Chris
  Meek, Sumit Gulwani, "Synchromesh: Reliable Code Generation from Pre-trained Language
  Models", ICLR 2022. <https://www.microsoft.com/en-us/research/publication/synchromesh-reliable-code-generation-from-pre-trained-language-models/>
  (OpenReview: <https://openreview.net/forum?id=KmtVD97J43e>)
- Demonstrated: constrained semantic decoding restricts model output to valid programs
  in the target language, combined with retrieval of similar examples; evaluated on
  SQL, Vega-Lite and SMCalFlow.
- Reported result: complementary gains from both components in accuracy and in
  avoiding runtime errors; see paper for figures.
- Limitations: guarantees validity with respect to the constraint, not correctness.
- Transfers: little. Constrained decoding helps generate well-formed artifacts
  (queries, configuration), not correct semantics.
- Does not transfer: FabricO11y does not generate code or queries with a model at
  runtime.
- Inference: no current use. Recorded so the distinction between well-formed and
  correct is not lost.

---

## Implications for FabricO11y verification

These are candidate practices, each labelled with its status in this repository.
"Current practice" reflects the repository after the architecture-foundation
milestone (updated after the digest was drafted), not a new guarantee. Anything marked "proposed" needs an ADR before adoption.

| Practice | Status | Main supporting sources |
| --- | --- | --- |
| Independent executable oracles, written in a different language (Python) and frozen before the implementation they grade | current practice | Zhao et al.; McKeeman; Bornholt et al. |
| State the independence assumptions per oracle (author, inputs seen, written before or after the implementation) | proposed | Zhao et al.; McKeeman |
| At least one negative control per contract: hand-written semantic mutants that the oracle or tests must reject | current practice: oracle mutation controls and the semantic-mutant registry (`xtask/mutants.json`), not yet per clause | Vikram et al.; Foster et al. |
| One argued non-equivalent semantic mutant per contract clause | proposed | Foster et al. |
| Generic mutation testing (cargo-mutants) on the pure semantic core | not adopted | cargo-mutants |
| Retain counterexamples from oracles and TLC as permanent fixtures | current practice ([ADR-0018](../decisions/ADR-0018-accept-work-on-executable-evidence.md)) | Jha et al.; Csmith |
| Protect oracle, TLA+ specification and fixture files from implementation patches; review any diff that touches both as suspicious | current practice as a rule ([ADR-0018](../decisions/ADR-0018-accept-work-on-executable-evidence.md) trust-boundary changes); not mechanically enforced | ImpossibleBench; Lean proof validation |
| Record TLC configuration (constants, constraints, checked properties) with each model-check claim | proposed | Lean proof validation |
| Metamorphic relations for query and rate semantics | planned; relations listed in the [verification strategy](../formal/verification-strategy.md), mostly unchecked | Chen et al. |
| Trace validation of the Rust delivery path against the delivery TLA+ model | proposed | Cirstea et al.; Davis et al. |
| Constrained decoding for generated artifacts | not adopted | Poesia et al. |

Open limitations of this digest:

- Rates from ImpossibleBench and ACH describe their settings; they are not predictions
  for this repository.
- Independence between a Python oracle and a Rust implementation is reduced when the
  same model family writes both from the same contract text. None of the sources
  measures that residual correlation directly.
