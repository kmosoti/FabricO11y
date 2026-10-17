"""Independent controls for the registered engineering-evidence v1 contract.

Origin: engineering-evidence-protocol.md, frozen in a47e1e3. Literal receipts
follow resource_group.py's emitted outer/command shapes. Every named mutation
records the violated join; production validators do not construct the oracle.
These fixtures attest finite structural controls, never actual job execution.
"""
import copy
import hashlib
import itertools
import json
import os
from pathlib import Path
import random
import tempfile
import unittest
from unittest import mock

import evidence


CANDIDATE = "a" * 40
HARNESS = "b" * 40
COMMAND = ["python3", "registered-control.py", "--case", "literal"]


def encoded(value):
    return json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode("utf-8")


def digest(raw):
    return hashlib.sha256(raw).hexdigest()


def permutation_order(jobs):
    """Independent finite oracle: enumerate total orders, test every constraint."""
    ids = sorted(job["id"] for job in jobs)
    edges = [(dependency, job["id"]) for job in jobs for dependency in job["needs"]]
    for order in itertools.permutations(ids):
        positions = {name: index for index, name in enumerate(order)}
        if all(positions[before] < positions[after] for before, after in edges):
            return list(order)
    raise ValueError("no legal total ordering")


class StrictJsonControls(unittest.TestCase):
    def test_literal_utf8_object(self):
        self.assertEqual(evidence.strict_json(b'{"text":"\xc3\xa9","items":[1,true,null]}'),
                         {"text": "é", "items": [1, True, None]})

    def test_malformed_duplicate_nonfinite_and_nonobject(self):
        controls = {
            "duplicate-root-key": b'{"id":1,"id":2}',
            "duplicate-nested-key": b'{"x":{"id":1,"id":2}}',
            "nonfinite-nan": b'{"x":NaN}',
            "nonfinite-positive": b'{"x":Infinity}',
            "nonfinite-negative": b'{"x":-Infinity}',
            "overflowed-float": b'{"x":1e9999}',
            "invalid-utf8": b'{"x":"\xff"}',
            "trailing-document": b'{} {}',
            "array-root": b'[]',
            "null-root": b'null',
            "string-root": b'"object"',
        }
        for origin, raw in controls.items():
            with self.subTest(origin=origin), self.assertRaises(ValueError):
                evidence.strict_json(raw)

    def test_size_and_depth_bounds(self):
        self.assertEqual(evidence.strict_json(b'{"x":' + b'[' * 8 + b'0' + b']' * 8 + b'}')["x"],
                         [[[[[[[[0]]]]]]]])
        for origin, raw in {
            "more-than-4MiB": b'{"x":"' + b'x' * (4 * 1024**2) + b'"}',
            "more-than-32-levels": b'{"x":' + b'[' * 40 + b'0' + b']' * 40 + b'}',
        }.items():
            with self.subTest(origin=origin), self.assertRaises(ValueError):
                evidence.strict_json(raw)


class GraphControls(unittest.TestCase):
    def test_all_directed_graphs_through_four_vertices(self):
        # No self edges in this family; the explicit self-edge control below
        # covers those. There are 4,165 labeled graphs across these finite sizes.
        checked = 0
        for count in range(1, 5):
            names = list("abcd"[:count])
            possible = [(source, destination) for source in names for destination in names
                        if source != destination]
            for mask in range(1 << len(possible)):
                jobs = [{"id": name, "needs": [source for bit, (source, destination)
                         in enumerate(possible) if destination == name and mask & (1 << bit)]}
                        for name in names]
                try:
                    expected = permutation_order(jobs)
                except ValueError:
                    with self.assertRaises(ValueError, msg=f"n={count}, mask={mask}"):
                        evidence.ordered_jobs(jobs)
                else:
                    self.assertEqual(evidence.ordered_jobs(jobs), expected,
                                     f"n={count}, mask={mask}")
                checked += 1
        self.assertEqual(checked, 4165)

    def test_seed17_input_order_does_not_change_output(self):
        jobs = [{"id": "a", "needs": []}, {"id": "b", "needs": []},
                {"id": "c", "needs": ["b", "a"]}, {"id": "d", "needs": ["a"]}]
        expected = permutation_order(jobs)
        rng = random.Random(17)
        for _ in range(40):
            shuffled = copy.deepcopy(jobs)
            rng.shuffle(shuffled)
            for job in shuffled:
                rng.shuffle(job["needs"])
            self.assertEqual(evidence.ordered_jobs(shuffled), expected)

    def test_invalid_graph_metadata(self):
        cases = {
            "empty": [],
            "duplicate-id": [{"id": "a", "needs": []}, {"id": "a", "needs": []}],
            "unknown-edge": [{"id": "a", "needs": ["missing"]}],
            "duplicate-edge": [{"id": "a", "needs": []}, {"id": "b", "needs": ["a", "a"]}],
            "self-edge": [{"id": "a", "needs": ["a"]}],
            "cycle": [{"id": "a", "needs": ["b"]}, {"id": "b", "needs": ["a"]}],
            "invalid-id": [{"id": "Upper", "needs": []}],
            "overlong-id": [{"id": "a" * 81, "needs": []}],
            "needs-not-list": [{"id": "a", "needs": "a"}],
            "dependency-not-string": [{"id": "a", "needs": [False]}],
            "more-than-4096-jobs": [{"id": f"j{i}", "needs": []} for i in range(4097)],
        }
        # 258 vertices, all off-diagonal edges: 66,306 exceeds 65,536.
        names = [f"j{i}" for i in range(258)]
        cases["more-than-65536-edges"] = [{"id": name, "needs": [other for other in names
                                                                if other != name]}
                                             for name in names]
        for origin, jobs in cases.items():
            with self.subTest(origin=origin), self.assertRaises(ValueError):
                evidence.ordered_jobs(jobs)


class CompositionControls(unittest.TestCase):
    def setUp(self):
        # Fail closed without the launcher's disk-backed scratch admission.
        scratch = Path(os.environ["FABRIC_SCRATCH_ROOT"]).resolve(strict=True)
        self.temporary = tempfile.TemporaryDirectory(prefix="evidence-controls-", dir=scratch)
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.plan = {"version": 1, "candidate": CANDIDATE, "artifacts": [], "jobs": []}
        self.receipts = {}
        self.add_job("literal")

    def add_job(self, name, needs=None, expected_exit=0):
        unit = "fabric-work-" + hashlib.sha256(name.encode()).hexdigest()[:16]
        group = "/sys/fs/cgroup/user.slice/user-1000.slice/user@1000.service/app.slice/" + unit + ".service"
        resource = {
            "unit": unit, "command": COMMAND.copy(), "exit": expected_exit,
            "wrapper_exit": expected_exit, "stop_confirmed": True, "stop_error": None,
            "storage_root": str(self.root),
            "command_receipt": {"unit": unit, "command": COMMAND.copy(), "cgroup": group,
                                "state": "completed", "exit": expected_exit,
                                "resource_observation": "command_return_before_service_stop",
                                "resources": {"memory.peak": "4096", "memory.events": "oom 0"}},
            "started_unix": 1.0, "elapsed_s": 0.1, "temporary_removed": True,
            "memory_high_bytes": 17179869184, "memory_max_bytes": 21474836480,
            "swap_max_bytes": 0, "runtime_max_seconds": 1800,
        }
        wrapper = {"state": "completed", "command": COMMAND.copy(),
                   "harness_revision": HARNESS, "exit": expected_exit,
                   "launcher_exit": expected_exit, "source_status_before": "",
                   "source_status_after": "", "resource_receipts": [
                       {"path": str(self.root / (name + "-resource.json")), "sha256": "",
                        "exit": expected_exit, "stop_confirmed": True}]}
        for kind, receipt in (("resource", resource), ("wrapper", wrapper)):
            artifact_id = name + "-" + kind
            self.receipts[artifact_id] = receipt
            self.plan["artifacts"].append({"id": artifact_id,
                                           "path": artifact_id + ".json", "sha256": ""})
        job = {"id": name, "needs": needs or [], "command": COMMAND.copy(),
               "harness_revision": HARNESS, "expected_exit": expected_exit,
               "evidence": {"wrapper": name + "-wrapper", "resource": name + "-resource"}}
        self.plan["jobs"].append(job)
        return job

    def publish(self):
        # Fixture producer deliberately computes fresh pins after mutations.
        # This makes semantic refusals independent of hash mismatch refusals.
        for kind in ("resource", "wrapper"):
            for artifact in self.plan["artifacts"]:
                if artifact["id"] not in self.receipts or not artifact["id"].endswith("-" + kind):
                    continue
                receipt = self.receipts[artifact["id"]]
                if kind == "wrapper":
                    resource_id = artifact["id"][:-len("wrapper")] + "resource"
                    resource_pin = next(item["sha256"] for item in self.plan["artifacts"]
                                        if item["id"] == resource_id)
                    receipt["resource_receipts"][0]["sha256"] = resource_pin
                raw = encoded(receipt)
                path = self.root / artifact["path"]
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(raw)
                artifact["sha256"] = digest(raw)
        raw = encoded(self.plan)
        (self.root / "plan.json").write_bytes(raw)
        return digest(raw)

    def verify(self, pin=None, candidate=CANDIDATE, plan_path="plan.json", root=None):
        pin = self.publish() if pin is None else pin
        return evidence.verify_plan(root or self.root, plan_path, pin, candidate)

    def assert_states(self, result, status, states):
        self.assertEqual(result["status"], status)
        self.assertEqual([(item["id"], item["state"]) for item in result["jobs"]], states)

    def test_literal_completed_positive_and_nonzero_exit_join(self):
        self.assert_states(self.verify(), "matched", [("literal", "matched")])
        self.add_job("refusal", ["literal"], expected_exit=7)
        self.assert_states(self.verify(), "matched", [("literal", "matched"), ("refusal", "matched")])

    def test_optional_child_revision_requires_exact_harness_pin(self):
        child = self.receipts["literal-resource"]["command_receipt"]
        child["revision"] = HARNESS
        self.assert_states(self.verify(), "matched", [("literal", "matched")])
        child["revision"] = "c" * 40
        with self.assertRaises(ValueError):
            self.verify()

    def test_pending_blocked_and_unmet_evidence_dependency(self):
        self.plan["jobs"][0]["evidence"] = None
        second = self.add_job("second", ["literal"])
        second["evidence"] = None
        third = self.add_job("third", ["second"])
        third["evidence"] = None
        self.assert_states(self.verify(), "pending", [("literal", "pending"),
                           ("second", "blocked"), ("third", "blocked")])
        third["evidence"] = {"wrapper": "third-wrapper", "resource": "third-resource"}
        with self.assertRaises(ValueError):
            self.verify()

    def test_stale_candidate_plan_and_artifact_hashes(self):
        pin = self.publish()
        with self.assertRaises(ValueError):
            self.verify(pin=pin, candidate="c" * 40)
        with self.assertRaises(ValueError):
            self.verify(pin="0" * 64)
        (self.root / "literal-resource.json").write_bytes(b'{}')
        with self.assertRaises(ValueError):
            self.verify(pin=pin)

    def test_unreferenced_artifact_is_still_hashed(self):
        self.plan["artifacts"].append({"id": "prerequisite", "path": "pin.bin", "sha256": digest(b"original")})
        (self.root / "pin.bin").write_bytes(b"changed")
        with self.assertRaises(ValueError):
            self.verify()

    def test_receipt_join_mutations(self):
        # Each tuple is a minimized, named protocol counterexample; hashes are
        # refreshed so an unrelated hash refusal cannot satisfy the control.
        cases = [
            ("wrapper-incomplete", "wrapper", ("state",), "started"),
            ("wrapper-wrong-argv", "wrapper", ("command",), ["different"]),
            ("resource-wrong-argv", "resource", ("command",), ["different"]),
            ("child-wrong-argv", "resource", ("command_receipt", "command"), ["different"]),
            ("wrapper-wrong-harness", "wrapper", ("harness_revision",), "c" * 40),
            ("wrapper-wrong-exit", "wrapper", ("exit",), 7),
            ("launcher-wrong-exit", "wrapper", ("launcher_exit",), 7),
            ("resource-wrong-exit", "resource", ("exit",), 7),
            ("resource-wrong-wrapper-exit", "resource", ("wrapper_exit",), 7),
            ("child-wrong-exit", "resource", ("command_receipt", "exit"), 7),
            ("wrapper-bool-exit", "wrapper", ("exit",), False),
            ("launcher-bool-exit", "wrapper", ("launcher_exit",), False),
            ("resource-bool-exit", "resource", ("exit",), False),
            ("resource-bool-wrapper-exit", "resource", ("wrapper_exit",), False),
            ("child-bool-exit", "resource", ("command_receipt", "exit"), False),
            ("dirty-before", "wrapper", ("source_status_before",), " M src/lib.rs"),
            ("dirty-after", "wrapper", ("source_status_after",), "?? surprise"),
            ("resource-not-stopped", "resource", ("stop_confirmed",), False),
            ("resource-integer-stop", "resource", ("stop_confirmed",), 1),
            ("resource-stop-error", "resource", ("stop_error",), "timeout"),
            ("storage-root-mismatch", "resource", ("storage_root",), "/somewhere-else"),
            ("invalid-unit", "resource", ("unit",), "other.service"),
            ("child-unit-mismatch", "resource", ("command_receipt", "unit"), "fabric-work-abcdef"),
            ("child-incomplete", "resource", ("command_receipt", "state"), "started"),
            ("relative-cgroup", "resource", ("command_receipt", "cgroup"), "user.slice/fabric-work-abc.service"),
            ("wrong-cgroup-root", "resource", ("command_receipt", "cgroup"), "/var/tmp/fabric-work-abc.service"),
            ("wrong-cgroup-unit", "resource", ("command_receipt", "cgroup"), "/sys/fs/cgroup/fabric-work-abc.service"),
        ]
        originals = copy.deepcopy(self.receipts)
        for origin, kind, keys, value in cases:
            with self.subTest(origin=origin):
                self.receipts = copy.deepcopy(originals)
                target = self.receipts["literal-" + kind]
                for key in keys[:-1]:
                    target = target[key]
                target[keys[-1]] = value
                with self.assertRaises(ValueError):
                    self.verify()

    def test_wrapper_resource_entry_mutations(self):
        self.publish()
        wrapper = self.receipts["literal-wrapper"]
        original = copy.deepcopy(wrapper["resource_receipts"])
        for origin, key, value in [("wrong-path", "path", "other.json"),
                                   ("wrong-entry-exit", "exit", 7),
                                   ("boolean-entry-exit", "exit", False),
                                   ("false-entry-stop", "stop_confirmed", False),
                                   ("integer-entry-stop", "stop_confirmed", 1)]:
            with self.subTest(origin=origin):
                wrapper["resource_receipts"] = copy.deepcopy(original)
                wrapper["resource_receipts"][0][key] = value
                with self.assertRaises(ValueError):
                    self.verify()
        for entries in ([], original + original):
            wrapper["resource_receipts"] = entries
            # Empty entries cannot use the fixture publisher's convenience join.
            pin = self.publish_raw_receipts()
            with self.assertRaises(ValueError):
                self.verify(pin=pin)

    def publish_raw_receipts(self):
        for artifact in self.plan["artifacts"]:
            raw = encoded(self.receipts[artifact["id"]])
            (self.root / artifact["path"]).write_bytes(raw)
            artifact["sha256"] = digest(raw)
        raw = encoded(self.plan)
        (self.root / "plan.json").write_bytes(raw)
        return digest(raw)

    def test_resource_entry_hash_mismatch(self):
        self.publish()
        self.receipts["literal-wrapper"]["resource_receipts"][0]["sha256"] = "0" * 64
        pin = self.publish_raw_receipts()
        with self.assertRaises(ValueError):
            self.verify(pin=pin)

    def test_noncanonical_cgroup_aliases(self):
        child = self.receipts["literal-resource"]["command_receipt"]
        correct = child["cgroup"]
        for origin, path in {"dot": correct.replace("/user.slice/", "/user.slice/./"),
                             "parent": correct.replace("/user.slice/", "/user.slice/../user.slice/"),
                             "empty": correct.replace("/user.slice/", "//user.slice/")}.items():
            with self.subTest(origin=origin), self.assertRaises(ValueError):
                child["cgroup"] = path
                self.verify()

    def test_shared_receipts_and_copied_wrong_job(self):
        second = self.add_job("second")
        for field in ("wrapper", "resource"):
            original = second["evidence"][field]
            second["evidence"][field] = "literal-" + field
            with self.subTest(origin="shared-" + field), self.assertRaises(ValueError):
                self.verify()
            second["evidence"][field] = original
        second["command"] = ["different-registered-job"]
        self.receipts["second-resource"] = copy.deepcopy(self.receipts["literal-resource"])
        self.receipts["second-wrapper"] = copy.deepcopy(self.receipts["literal-wrapper"])
        with self.assertRaises(ValueError):
            self.verify()

    def test_identical_jobs_cannot_reuse_copied_resource_unit(self):
        # Prospective clarification: distinct file paths and refreshed hashes
        # cannot turn one launcher invocation into evidence for two jobs.
        self.add_job("second")
        self.receipts["second-resource"] = copy.deepcopy(self.receipts["literal-resource"])
        with self.assertRaises(ValueError):
            self.verify()

    def test_actual_delegated_workload_cgroup_shape(self):
        resource = self.receipts["literal-resource"]
        child = resource["command_receipt"]
        service_group = child["cgroup"]
        resource["delegated"] = True
        child["cgroup"] = service_group + "/workload"
        self.assert_states(self.verify(), "matched", [("literal", "matched")])
        for origin, delegated, path in [
            ("undelegated-workload", False, service_group + "/workload"),
            ("integer-delegation", 1, service_group + "/workload"),
            ("arbitrary-child", True, service_group + "/other"),
            ("nested-child", True, service_group + "/workload/nested"),
        ]:
            with self.subTest(origin=origin):
                resource["delegated"] = delegated
                child["cgroup"] = path
                with self.assertRaises(ValueError):
                    self.verify()

    def test_plan_schema_mutations(self):
        original = copy.deepcopy(self.plan)
        mutations = [
            ("unknown-plan", lambda p: p.update(extra=True)),
            ("wrong-version", lambda p: p.update(version=2)),
            ("boolean-version", lambda p: p.update(version=True)),
            ("uppercase-candidate", lambda p: p.update(candidate="A" * 40)),
            ("unknown-job", lambda p: p["jobs"][0].update(extra=True)),
            ("unknown-artifact", lambda p: p["artifacts"][0].update(extra=True)),
            ("unknown-evidence", lambda p: p["jobs"][0]["evidence"].update(extra=True)),
            ("missing-resource-id", lambda p: p["jobs"][0]["evidence"].update(resource="absent")),
            ("duplicate-artifact-id", lambda p: p["artifacts"][1].update(id=p["artifacts"][0]["id"])),
            ("duplicate-artifact-path", lambda p: p["artifacts"][1].update(path=p["artifacts"][0]["path"])),
            ("bool-expected-exit", lambda p: p["jobs"][0].update(expected_exit=False)),
            ("negative-exit", lambda p: p["jobs"][0].update(expected_exit=-1)),
            ("large-exit", lambda p: p["jobs"][0].update(expected_exit=256)),
            ("empty-command", lambda p: p["jobs"][0].update(command=[])),
            ("empty-argument", lambda p: p["jobs"][0].update(command=[""])),
            ("nonstring-argument", lambda p: p["jobs"][0].update(command=[1])),
            ("257-arguments", lambda p: p["jobs"][0].update(command=["x"] * 257)),
            ("overlong-argument", lambda p: p["jobs"][0].update(command=["x" * (1024**2 + 1)])),
            ("overlong-utf8-argument", lambda p: p["jobs"][0].update(command=["é" * (512 * 1024 + 1)])),
            ("unknown-prerequisite", lambda p: p["jobs"][0].update(needs=["absent"])),
        ]
        # Publish valid receipts first, then mutate only plan bytes to isolate
        # schema checking from fixture producer assumptions.
        self.publish()
        for origin, mutate in mutations:
            with self.subTest(origin=origin):
                self.plan = copy.deepcopy(original)
                # Recover actual pins produced above before applying mutation.
                for artifact in self.plan["artifacts"]:
                    artifact["sha256"] = digest((self.root / artifact["path"]).read_bytes())
                mutate(self.plan)
                raw = encoded(self.plan)
                (self.root / "plan.json").write_bytes(raw)
                with self.assertRaises(ValueError):
                    self.verify(pin=digest(raw))

    def test_required_receipt_fields_cannot_be_omitted(self):
        self.publish()
        originals = copy.deepcopy(self.receipts)
        cases = [("wrapper", (field,)) for field in
                 ("state", "command", "harness_revision", "exit", "launcher_exit",
                  "source_status_before", "source_status_after", "resource_receipts")]
        cases += [("resource", (field,)) for field in
                  ("unit", "command", "exit", "wrapper_exit", "stop_confirmed",
                   "stop_error", "storage_root", "command_receipt")]
        cases += [("resource", ("command_receipt", field))
                  for field in ("unit", "command", "cgroup", "state", "exit")]
        for kind, keys in cases:
            with self.subTest(kind=kind, missing=keys):
                self.receipts = copy.deepcopy(originals)
                target = self.receipts["literal-" + kind]
                for key in keys[:-1]:
                    target = target[key]
                del target[keys[-1]]
                pin = self.publish_raw_receipts()
                with self.assertRaises(ValueError):
                    self.verify(pin=pin)

    def test_missing_evidence_and_nonobject_receipts(self):
        pin = self.publish()
        (self.root / "literal-resource.json").unlink()
        with self.assertRaises((ValueError, OSError)):
            self.verify(pin=pin)
        for raw in (b'[]', b'null', b'{"duplicate":1,"duplicate":2}'):
            self.publish()
            path = self.root / "literal-resource.json"
            path.write_bytes(raw)
            self.plan["artifacts"][0]["sha256"] = digest(raw)
            plan_raw = encoded(self.plan)
            (self.root / "plan.json").write_bytes(plan_raw)
            with self.subTest(raw=raw), self.assertRaises(ValueError):
                self.verify(pin=digest(plan_raw))

    def test_noncanonical_artifact_and_plan_paths(self):
        self.publish()
        for bad in ("../outside", "./literal-resource.json", "sub/../literal-resource.json",
                    "sub//file", "/absolute", "sub/", ""):
            with self.subTest(path=bad):
                plan = copy.deepcopy(self.plan)
                plan["artifacts"][0]["path"] = bad
                raw = encoded(plan)
                (self.root / "plan.json").write_bytes(raw)
                with self.assertRaises((ValueError, OSError)):
                    self.verify(pin=digest(raw))
                with self.assertRaises((ValueError, OSError)):
                    self.verify(pin=digest(raw), plan_path=bad)

    def test_symlink_components_and_nonregular_artifacts(self):
        pin = self.publish()
        resource = self.root / "literal-resource.json"
        target = self.root / "held-resource.json"
        resource.rename(target)
        resource.symlink_to(target.name)
        with self.assertRaises((ValueError, OSError)):
            self.verify(pin=pin)
        resource.unlink()
        resource.mkdir()
        with self.assertRaises((ValueError, OSError)):
            self.verify(pin=pin)
        resource.rmdir()
        os.mkfifo(resource)
        with self.assertRaises((ValueError, OSError)):
            self.verify(pin=pin)
        resource.unlink()
        target.rename(resource)
        (self.root / "linked").symlink_to(self.root, target_is_directory=True)
        self.plan["artifacts"][0]["path"] = "linked/literal-resource.json"
        raw = encoded(self.plan)
        (self.root / "plan.json").write_bytes(raw)
        with self.assertRaises((ValueError, OSError)):
            self.verify(pin=digest(raw))

    def test_symlink_plan_and_root_are_refused(self):
        pin = self.publish()
        (self.root / "alias-plan.json").symlink_to("plan.json")
        with self.assertRaises((ValueError, OSError)):
            self.verify(pin=pin, plan_path="alias-plan.json")
        alias = self.root / "alias-root"
        alias.symlink_to(self.root, target_is_directory=True)
        with self.assertRaises((ValueError, OSError)):
            self.verify(pin=pin, root=alias)
        relative = Path(os.path.relpath(self.root))
        with self.assertRaises((ValueError, OSError)):
            self.verify(pin=pin, root=relative)

    def test_artifact_metadata_size_bound_before_streaming(self):
        self.publish()
        # Sparse owned fixture consumes metadata only; never hash 8 GiB to test
        # an admission limit. A read attempt means the bound was checked late.
        giant = self.root / "oversize.bin"
        with giant.open("wb") as stream:
            stream.truncate(8 * 1024**3 + 1)
        self.plan["artifacts"].insert(0, {"id": "oversize", "path": "oversize.bin", "sha256": "0" * 64})
        raw = encoded(self.plan)
        (self.root / "plan.json").write_bytes(raw)
        real_read = os.read
        giant_identity = (giant.stat().st_dev, giant.stat().st_ino)

        def guarded_read(fd, count):
            stat = os.fstat(fd)
            if (stat.st_dev, stat.st_ino) == giant_identity:
                self.fail("oversized artifact was streamed before metadata refusal")
            return real_read(fd, count)

        with mock.patch.object(evidence.os, "read", side_effect=guarded_read):
            with self.assertRaises(ValueError):
                self.verify(pin=digest(raw))

    def test_aggregate_retained_json_budget_includes_separate_documents(self):
        # The production 64 MiB budget is patched down to these literal small
        # documents. This tests composition without allocating oversized input.
        pin = self.publish()
        sizes = [(self.root / name).stat().st_size for name in
                 ("plan.json", "literal-resource.json", "literal-wrapper.json")]
        total = sum(sizes)
        self.assertLess(max(sizes), total - 1)
        with mock.patch.object(evidence, "JSON_TOTAL_LIMIT", total):
            self.assert_states(self.verify(pin=pin), "matched", [("literal", "matched")])
        with mock.patch.object(evidence, "JSON_TOTAL_LIMIT", total - 1):
            with self.assertRaises(ValueError):
                self.verify(pin=pin)

    def test_each_artifact_streamed_once_and_parse_uses_held_bytes(self):
        pin = self.publish()
        tracked = {}
        for name in ("literal-resource.json", "literal-wrapper.json"):
            stat = (self.root / name).stat()
            tracked[(stat.st_dev, stat.st_ino)] = [stat.st_size, 0]
        real_read = os.read

        def counting_read(fd, count):
            stat = os.fstat(fd)
            raw = real_read(fd, count)
            key = (stat.st_dev, stat.st_ino)
            if key in tracked:
                tracked[key][1] += len(raw)
            return raw

        with mock.patch.object(evidence.os, "read", side_effect=counting_read):
            self.assert_states(self.verify(pin=pin), "matched", [("literal", "matched")])
        for expected_bytes, actual_bytes in tracked.values():
            self.assertEqual(actual_bytes, expected_bytes)

    def test_replacement_and_inplace_write_after_artifact_read(self):
        # Deterministic race origin: contract's final descriptor/path identity
        # recheck. Change after EOF of the first artifact's original descriptor,
        # while returning exactly the original bytes to its hash/parser.
        for mutation in ("replace", "inplace"):
            with self.subTest(origin=mutation):
                pin = self.publish()
                path = self.root / "literal-resource.json"
                initial = path.stat()
                identity = (initial.st_dev, initial.st_ino)
                original = path.read_bytes()
                real_read = os.read
                triggered = []

                def racing_read(fd, count):
                    stat = os.fstat(fd)
                    raw = real_read(fd, count)
                    if not raw and not triggered and (stat.st_dev, stat.st_ino) == identity:
                        triggered.append(True)
                        if mutation == "replace":
                            replacement = self.root / "replacement.json"
                            replacement.write_bytes(original)
                            replacement.replace(path)
                        else:
                            # Equal-size bytes and restored mtime still leave
                            # ctime changed; a size/mtime-only check is weak.
                            path.write_bytes(original.replace(b'"elapsed_s":0.1', b'"elapsed_s":0.2'))
                            os.utime(path, ns=(initial.st_atime_ns, initial.st_mtime_ns))
                    return raw

                with mock.patch.object(evidence.os, "read", side_effect=racing_read):
                    with self.assertRaises((ValueError, OSError)):
                        self.verify(pin=pin)
                self.assertTrue(triggered, "race hook must observe artifact descriptor EOF")


if __name__ == "__main__":
    unittest.main()
