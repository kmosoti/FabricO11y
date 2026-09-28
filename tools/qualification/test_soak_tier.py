"""Negative controls for the soak decision rule: each injected defect must fail
exactly the gate that names it, and a clean trial must pass every gate."""

import copy
import unittest

from soak_tier import IDENTITIES, WINDOWS, _count_le, evaluate


def clean():
    return {
        "oracle_passed": True, "sim_exit": 0, "server_exit": 0,
        "ack_ms_by_window": [[60.0] * 100 for _ in range(WINDOWS)],
        "backlog": [IDENTITIES, IDENTITIES + 3, IDENTITIES],
        "server_vmhwm_kib": 300 * 1024,
        "rss_kib_by_window": [[200 * 1024] * 10 for _ in range(WINDOWS)],
        "query_s": [0.1] * 500, "query_failed": 0, "query_incomplete": 0,
        "management_ok": 90, "management_failed": 0,
        "sealed_left": 0, "segments": 10,
    }


def with_(**changes):
    m = copy.deepcopy(clean())
    m.update(changes)
    return m


class SoakDecisionRule(unittest.TestCase):
    def failed(self, m):
        return sorted(k for k, v in evaluate(m).items() if not v)

    def test_clean_trial_passes_every_gate(self):
        self.assertEqual(self.failed(clean()), [])

    def test_each_injected_defect_fails_its_gate(self):
        slow = clean()["ack_ms_by_window"]
        slow[WINDOWS - 1] = [60.0] * 90 + [1500.0] * 10
        missing = clean()["ack_ms_by_window"][:-1]
        empty = clean()["ack_ms_by_window"]
        empty[3] = []
        leak = clean()["rss_kib_by_window"]
        leak[-1] = [600 * 1024] * 10
        cases = {
            "oracle": with_(oracle_passed=False),
            "exits": with_(server_exit=1),
            "ack_p99_le_1s_every_window": [with_(ack_ms_by_window=slow),
                                           with_(ack_ms_by_window=missing),
                                           with_(ack_ms_by_window=empty)],
            "no_growing_backlog": [with_(backlog=[10, 50, 10 + IDENTITIES + 1]), with_(backlog=[])],
            "server_rss_le_2gib": with_(server_vmhwm_kib=2 * 1024 * 1024 + 1),
            "no_rss_growth": with_(rss_kib_by_window=leak),
            "queries_complete": [with_(query_failed=1), with_(query_incomplete=1)],
            "query_p99_le_2s": with_(query_s=[0.1] * 98 + [2.5] * 2),
            "management_ok": [with_(management_failed=1), with_(management_ok=0)],
            "sealing_caught_up": [with_(sealed_left=1), with_(segments=0)],
        }
        for gate, ms in cases.items():
            for m in ms if isinstance(ms, list) else [ms]:
                with self.subTest(gate=gate):
                    self.assertEqual(self.failed(m), [gate])

    def test_boundaries_are_inclusive(self):
        at_limit = clean()["ack_ms_by_window"]
        at_limit[0] = [1000.0] * 100
        self.assertEqual(self.failed(with_(ack_ms_by_window=at_limit,
                                           backlog=[5, 5 + IDENTITIES],
                                           server_vmhwm_kib=2 * 1024 * 1024,
                                           query_s=[2.0] * 10)), [])

    def test_count_le(self):
        self.assertEqual([_count_le([1, 2, 2, 5], v) for v in (0, 1, 2, 4, 5, 9)], [0, 1, 3, 3, 4, 4])


if __name__ == "__main__":
    unittest.main()
