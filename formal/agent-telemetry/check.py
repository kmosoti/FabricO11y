#!/usr/bin/env python3
"""Z3 merge laws plus a bounded, independent check against the Python reducer."""

from dataclasses import dataclass, replace
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import platform
import sys

import z3

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]


@dataclass(frozen=True)
class Evidence:
    started: object
    finished: object
    name_known: object
    name: object
    code_known: object
    code: object
    rejected: object


def symbolic(prefix):
    return Evidence(z3.Bool(prefix + "_started"), z3.Bool(prefix + "_finished"),
                    z3.Bool(prefix + "_name_known"), z3.Int(prefix + "_name"),
                    z3.Bool(prefix + "_code_known"), z3.Int(prefix + "_code"),
                    z3.Bool(prefix + "_rejected"))


EMPTY = Evidence(False, False, False, 0, False, 0, False)


def well_formed(value):
    # A name exists exactly when at least one operation observation exists;
    # a known code can only come from a finish observation.
    return z3.And(value.name_known == z3.Or(value.started, value.finished),
                  z3.Implies(value.code_known, value.finished))


def merge(left, right):
    return Evidence(
        z3.Or(left.started, right.started),
        z3.Or(left.finished, right.finished),
        z3.Or(left.name_known, right.name_known),
        z3.If(left.name_known, left.name, right.name),
        z3.Or(left.code_known, right.code_known),
        z3.If(left.code_known, left.code, right.code),
        z3.Or(left.rejected, right.rejected,
              z3.And(left.name_known, right.name_known, left.name != right.name),
              z3.And(left.code_known, right.code_known, left.code != right.code)),
    )


def equivalent(left, right):
    # Python raises rather than returning a partial snapshot after conflict.
    # All rejected states therefore represent the same observable outcome.
    return z3.And(
        left.rejected == right.rejected,
        z3.Implies(z3.Not(left.rejected), z3.And(
            left.started == right.started, left.finished == right.finished,
            left.name_known == right.name_known,
            z3.Implies(left.name_known, left.name == right.name),
            left.code_known == right.code_known,
            z3.Implies(left.code_known, left.code == right.code))),
    )


def preserves_known(left, right, result):
    return z3.Implies(z3.Not(result.rejected), z3.And(
        z3.Implies(left.code_known, z3.And(result.code_known, result.code == left.code)),
        z3.Implies(right.code_known, z3.And(result.code_known, result.code == right.code))))


def code_has_evidence(left, right, result):
    return z3.Implies(z3.And(z3.Not(result.rejected), result.code_known), z3.Or(
        z3.And(left.code_known, result.code == left.code),
        z3.And(right.code_known, result.code == right.code)))


def code_conflict_rejected(left, right, result):
    return z3.Implies(z3.And(left.code_known, right.code_known, left.code != right.code),
                      result.rejected)


def solver():
    instance = z3.Solver()
    instance.set(timeout=10_000, random_seed=0)
    return instance


def query(name, assumptions, property_, expected="unsat", witnesses=None):
    if expected not in {"sat", "unsat"}:
        raise ValueError("Only sat/unsat are acceptable solver outcomes")
    instance = solver()
    instance.add(*assumptions)
    domain = instance.check()
    if domain != z3.sat:
        return {"name": name, "assumptions": str(domain), "passed": False,
                "error": "Assumptions must be satisfiable; no vacuous proofs."}
    instance.add(z3.Not(property_))
    result = instance.check()
    record = {"name": name, "assumptions": "sat", "negated_property": str(result),
              "expected": expected, "passed": str(result) == expected}
    if result == z3.sat:
        model = instance.model()
        if witnesses is None:
            record["counterexample"] = {str(symbol): str(model[symbol]) for symbol in model.decls()}
        else:
            record["counterexample"] = {
                label: str(model.eval(term, model_completion=True)) for label, term in witnesses.items()}
    elif result == z3.unknown:
        record["reason_unknown"] = instance.reason_unknown()
    return record


def symbolic_checks():
    a, b, c = (symbolic(prefix) for prefix in ("a", "b", "c"))
    domain = [well_formed(a), well_formed(b), well_formed(c)]
    merged = merge(a, b)
    laws = [
        ("identity", equivalent(merge(a, EMPTY), a)),
        ("idempotence", equivalent(merge(a, a), a)),
        ("commutativity", equivalent(merged, merge(b, a))),
        ("associativity", equivalent(merge(merge(a, b), c), merge(a, merge(b, c)))),
        ("closed_well_formed", well_formed(merged)),
        ("preserve_known_code", preserves_known(a, b, merged)),
        ("no_fabricated_code", code_has_evidence(a, b, merged)),
        ("reject_conflicting_codes", code_conflict_rejected(a, b, merged)),
        ("reject_conflicting_names", z3.Implies(
            z3.And(a.name_known, b.name_known, a.name != b.name), merged.rejected)),
        ("flags_monotonic", z3.And(
            z3.Implies(a.started, merged.started), z3.Implies(b.started, merged.started),
            z3.Implies(a.finished, merged.finished), z3.Implies(b.finished, merged.finished))),
        ("rejection_absorbing", z3.Implies(z3.Or(a.rejected, b.rejected), merged.rejected)),
        ("no_false_rejection", z3.Implies(z3.And(
            z3.Not(a.rejected), z3.Not(b.rejected),
            z3.Implies(z3.And(a.name_known, b.name_known), a.name == b.name),
            z3.Implies(z3.And(a.code_known, b.code_known), a.code == b.code)),
            z3.Not(merged.rejected))),
    ]
    results = [query(name, domain, law) for name, law in laws]

    # Pointwise map updates: the key is the complete (turn, operation) pair.
    key_type = z3.Datatype("TelemetryOperationKey")
    key_type.declare("key", ("turn", z3.IntSort()), ("operation", z3.IntSort()))
    key_type = key_type.create()
    turn_a, turn_b, op_a, op_b = z3.Ints("turn_a turn_b op_a op_b")
    key_a, key_b = key_type.key(turn_a, op_a), key_type.key(turn_b, op_b)
    states = z3.Array("states", key_type, z3.IntSort())
    new_value = z3.Int("new_value")
    results.append(query("other_operation_unchanged", [key_a != key_b],
                         z3.Select(z3.Store(states, key_a, new_value), key_b) == z3.Select(states, key_b)))

    # Negative controls use the same preservation/evidence/conflict properties.
    overwrite = replace(merged, code_known=b.code_known, code=b.code)
    suppress_conflict = replace(merged, rejected=z3.Or(a.rejected, b.rejected))
    fabricate_zero = replace(merged, code_known=True, code=z3.If(merged.code_known, merged.code, 0))
    inputs = {prefix + "." + field: term for prefix, value in (("a", a), ("b", b))
              for field, term in vars(value).items()}
    controls = [
        query("mutant_overwrite_unknown", domain, preserves_known(a, b, overwrite), "sat", inputs),
        query("mutant_suppress_conflict", domain, code_conflict_rejected(a, b, suppress_conflict), "sat", inputs),
        query("mutant_fabricate_zero", domain, code_has_evidence(a, b, fabricate_zero), "sat", inputs),
    ]
    aliased = z3.Array("aliased_states", z3.IntSort(), z3.IntSort())
    controls.append(query("mutant_key_only_by_operation", [turn_a != turn_b, op_a == op_b],
                          z3.Select(z3.Store(aliased, op_a, new_value), op_b) == z3.Select(aliased, op_b), "sat"))
    return {"laws": results, "negative_controls": controls}


def load_module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, help="also save the complete JSON evidence report")
    args = parser.parse_args()
    source_paths = [HERE / "check.py", HERE / "check_implementation.py", HERE / "requirements.txt",
                    ROOT / "tools/telemetry/telemetry.py"]
    report = {"z3_version": z3.get_version_string(), "python_version": platform.python_version(),
              "source_sha256": {str(path.relative_to(ROOT)): hashlib.sha256(path.read_bytes()).hexdigest()
                                for path in source_paths}}
    report.update(symbolic_checks())
    try:
        reference = load_module("telemetry_formal_reference", HERE / "check_implementation.py")
        implementation = load_module("telemetry_under_formal_check", ROOT / "tools/telemetry/telemetry.py")
        report["implementation_check"] = reference.run_checks(implementation.reduce_events)
        report["implementation_check"]["passed"] = True
    except Exception as error:
        report["implementation_check"] = {"passed": False, "error": str(error)}
    report["passed"] = (all(row["passed"] for row in report["laws"] + report["negative_controls"])
                        and report["implementation_check"]["passed"])
    encoded = json.dumps(report, indent=2, sort_keys=True) + "\n"
    if args.output:
        args.output.write_text(encoded)
    print(encoded, end="")
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    sys.exit(main())
