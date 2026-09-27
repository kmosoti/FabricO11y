"""Specification-first E2R contract tests. Candidate is imported only in tests."""

from __future__ import annotations

import importlib.util
from hashlib import sha256
from pathlib import Path
import sys
import unittest

from oracle_support import (SECTORS, SEEDS, bodies, covered_byte_flips, fixture,
                            group_bytes, logical_append, persist_subset,
                            sector_subsets, supervisor_decision)


def candidate():
    path = Path(__file__).with_name("candidate.py")
    spec = importlib.util.spec_from_file_location("seal_candidate", path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def accepted(result, groups, prefix):
    return result == ("ok", groups, prefix)


def failed(result, original):
    return result == ("error", original)


class ReferenceChecks(unittest.TestCase):
    def test_frozen_fixture_hashes(self):
        expected = {
            512: "2e4ff2bfca3053e9ef8cb9d186ef9589e38a8987b293643a4bfaddcb3452661b",
            4096: "9ac8dd3a8868cf9d9c0c5c3683b9b93f211a699ac63ddb5746478dbf7b1fa9cb",
        }
        for sector, digest in expected.items():
            image = fixture((bodies(1, 11), bodies(6, 11)), sector)
            self.assertEqual(sha256(image).hexdigest(), digest)

    def test_fixture_shape_and_counts(self):
        masks = 0
        writes_and_syncs = 0
        sampled = 0
        exhaustive = 0
        for sector in SECTORS:
            for seed in SEEDS:
                for size in range(1, 7):
                    group = group_bytes(1, bodies(size, seed), sector)
                    self.assertEqual(len(group) % sector, 0)
                    count = len(group) // sector
                    n_masks = sum(1 for _ in sector_subsets(group, sector))
                    expected = 1 << count if count <= 16 else 100_000
                    self.assertEqual(n_masks, expected)
                    masks += n_masks
                    writes_and_syncs += count + 1
                    sampled += count > 16
                    exhaustive += count <= 16
        print(f"reference workload: {masks} subset cases; "
              f"{exhaustive} exhaustive fixtures; {sampled} sampled fixtures; "
              f"{writes_and_syncs} injected write/sync boundaries; "
              f"{len(SEEDS) * len(SECTORS) * 1000} covered-byte flips")

    def test_non_vacuity_probes(self):
        sector = 512
        prior = group_bytes(0, bodies(1, 11), sector)
        group = group_bytes(1, bodies(2, 11), sector)
        clean = prior + group
        self.assertFalse(accepted(("error", clean), (bodies(1, 11), bodies(2, 11)), len(clean)))
        changed = bytearray(prior)
        changed[sector + 12] ^= 1
        corrupt = bytes(changed) + group
        self.assertFalse(failed(("ok", tuple(), 0), corrupt))
        seal_only = prior + persist_subset(group, sector, 1 << (len(group) // sector - 1))
        self.assertFalse(failed(("ok", (bodies(1, 11),), len(prior)), seal_only))
        self.assertNotEqual(supervisor_decision(False, True),
                            supervisor_decision(True, True))


class CandidateChecks(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.impl = candidate()

    def test_clean_encode_and_recover(self):
        for sector in SECTORS:
            for seed in SEEDS:
                for size in range(1, 7):
                    groups = (bodies(1, seed), bodies(size, seed))
                    image = fixture(groups, sector)
                    with self.subTest(sector=sector, seed=seed, size=size):
                        self.assertEqual(self.impl.encode_groups(groups, sector), image)
                        self.assertTrue(accepted(self.impl.recover(image, sector), groups, len(image)))

    def test_persisted_sector_subsets(self):
        violations = 0
        cases = 0
        for sector in SECTORS:
            for seed in SEEDS:
                prior_bodies = bodies(1, seed)
                prior = group_bytes(0, prior_bodies, sector)
                for size in range(1, 7):
                    current_bodies = bodies(size, seed)
                    current = group_bytes(1, current_bodies, sector)
                    count = len(current) // sector
                    full = (1 << count) - 1
                    seal_bit = 1 << (count - 1)
                    for mask in sector_subsets(current, sector):
                        image = prior + persist_subset(current, sector, mask)
                        actual = self.impl.recover(image, sector)
                        if mask == full:
                            valid = accepted(actual, (prior_bodies, current_bodies), len(image))
                        elif mask & seal_bit:
                            valid = failed(actual, image)
                        else:
                            valid = (failed(actual, image) or
                                     accepted(actual, (prior_bodies,), len(prior)))
                        violations += not valid
                        cases += 1
                        if violations >= 20:
                            self.fail(f"at least {violations} subset violations after {cases} cases; "
                                      f"last sector={sector} seed={seed} size={size} mask={mask}")
        self.assertEqual(violations, 0, f"contract_violations={violations}/{cases}")

    def test_committed_covered_byte_flips(self):
        for sector in SECTORS:
            for seed in SEEDS:
                prior = group_bytes(0, bodies(6, seed), sector)
                later = group_bytes(1, bodies(1, seed), sector)
                for flipped, offset in covered_byte_flips(prior, sector, seed):
                    image = flipped + later
                    with self.subTest(sector=sector, seed=seed, offset=offset):
                        self.assertTrue(failed(self.impl.recover(image, sector), image))

    def test_error_boundaries_and_external_witness(self):
        for sector in SECTORS:
            for seed in SEEDS:
                previous = group_bytes(0, bodies(1, seed), sector)
                for size in range(1, 7):
                    current_bodies = bodies(size, seed)
                    group = group_bytes(1, current_bodies, sector)
                    expected = logical_append(previous, group, sector)
                    clean = self.impl.append_logical(previous, 1, current_bodies, sector, None)
                    with self.subTest(sector=sector, seed=seed, size=size, at=None):
                        self.assertEqual(clean, (expected.image, True, False, False,
                                                 expected.operations))
                    clean_scan = self.impl.recover(clean[0], sector)
                    self.assertTrue(accepted(clean_scan, (bodies(1, seed), current_bodies), len(clean[0])))
                    self.assertEqual(self.impl.supervise(False, clean_scan), "resume_allowed")
                    for at in expected.operations:
                        model = logical_append(previous, group, sector, at)
                        actual = self.impl.append_logical(previous, 1, current_bodies, sector, at)
                        with self.subTest(sector=sector, seed=seed, size=size, at=at):
                            self.assertEqual(actual, (model.image, False, True, True,
                                                      model.operations))
                            scan = self.impl.recover(actual[0], sector)
                            self.assertEqual(self.impl.supervise(True, scan),
                                             "rebuild_from_trusted_source")
                            if at == "sync":
                                self.assertEqual(actual[0], clean[0])
                                self.assertEqual(scan, clean_scan)


if __name__ == "__main__":
    unittest.main()
