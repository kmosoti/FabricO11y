# Reasoning skill evaluation

36 real Codex CLI calls completed: six task types, two repetitions, original,
tuned and no-supplied-method prompts. This is a prompt-level method ablation,
not native skill discovery/routing evaluation or a held-out generalization claim.
The same configured Codex runtime was used; caching/context variation confounds
a causal latency interpretation. No global skill was replaced.

Raw label grades were 10/12 in every arm. Inspection found all six proof answers
correctly denied whole-system proof, using not_equivalent rather than the rubric's
unsupported label. That is an ambiguous oracle, not six demonstrated model failures.
Raw artifacts remain unchanged. Do not use these scores as an adoption gate.

Tuned median input tokens: 18329 versus original 19982; median elapsed time
4.110 s versus 3.475 s. No-supplied-method median: 18105 tokens and 3.534 s.
These simple tasks do not establish a useful improvement. Retain the installed
original skill. The shorter candidate is available for follow-up native-routing
and held-out evaluation; its duplicated front matter is removed in the candidate.

The runner retains exits, elapsed times, usage and raw replies in its output directory. Before expanding evaluation,
fix ambiguous grading through explicit multi-label/rationale criteria and add
held-out variants. Do not silently rewrite historical grades.

A supplemental held-out wording set completed 18 calls (six variants × three
arms). All label checks matched (6/6 in each arm). The candidate was frozen
before these variants ran. This remains a supplied-method prompt evaluation;
the no-method arm does not disable inherited Codex skill discovery. No native
routing or statistically general performance improvement is established.
Decision: keep the shared original unchanged and retain the tuning candidate.
