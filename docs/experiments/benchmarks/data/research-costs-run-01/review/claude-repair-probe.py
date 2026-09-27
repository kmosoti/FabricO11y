#!/usr/bin/env python3
"""Bounded independent re-review of the repaired cost checker (new schema).

Uses the already-produced VALID smoke output at
/home/kmosoti/fabric-cost-repair-smoke-20260927-c (never the stale final-c
smoke, which predates the schema). Runs a positive control plus the
discriminating mutations GPT raised against the prior checker: reversed
layout gate via a bad p50, duplicate/missing query index, negative timing,
false match counts. Also independently re-verifies persisted bytes against
files on disk and the amortization (build+publication) formula. Writes only
under this directory; never touches implementation, tests or main.
"""
import copy
import json
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, '/home/kmosoti/fabric-cost-repair/tools/bench')
from run_research_costs import checked, summarize, validate_layout_artifacts, validate_pair  # noqa: E402

SMOKE = Path('/home/kmosoti/fabric-cost-repair-smoke-20260927-c')
FAILURES = []


def check(label, condition, detail=""):
    status = "OK" if condition else "FAIL"
    print(f"[{status}] {label} {detail}")
    if not condition:
        FAILURES.append(f"{label} {detail}")


def reject(label, operation):
    try:
        operation()
    except RuntimeError as exc:
        check(label, True, f"(correctly rejected: {exc})")
    else:
        check(label, False, "(accepted corrupted evidence)")


def main():
    source = SMOKE / 'mixed-201-2048-source.json'
    receipt = SMOKE / 'mixed-201-2048-0-receipt.json'
    layout = SMOKE / 'mixed-201-2048-0-json64' / 'result.json'
    zstd_dir = SMOKE / 'mixed-201-2048-0-zstd64'
    zstd = zstd_dir / 'result.json'
    source_hash = __import__('hashlib').sha256(source.read_bytes()).hexdigest()

    good = [json.loads(receipt.read_text()), json.loads(layout.read_text())]
    try:
        validate_pair(source_hash, good)
        checked(receipt, 'receipt', 'smoke', 'mixed', 201, 0, 256)
        checked(layout, 'layout', 'smoke', 'mixed', 201, 0, 256)
        good_zstd = checked(zstd, 'layout', 'smoke', 'mixed', 201, 0, 384)
        validate_layout_artifacts(zstd_dir, good_zstd)
        check("positive control: unmutated receipt/json64/zstd64 results pass checked()+validate_pair()+validate_layout_artifacts()", True)
    except RuntimeError as exc:
        check("positive control: unmutated results pass", False, str(exc))
        return 1

    reject("mutated p50_projected_ns no longer flips the layout gate silently",
           lambda: checked(_write(zstd_dir, good_zstd, {'p50_projected_ns': 100_000_000}), 'layout', 'smoke', 'mixed', 201, 0, 384))

    reject("duplicate sample index (overwriting sample 127 with sample 0) is rejected",
           lambda: checked(_write(zstd_dir, good_zstd, {'samples': _dup_first_over_last(good_zstd['samples'])}), 'layout', 'smoke', 'mixed', 201, 0, 384))

    reject("negative sample wall_ns is rejected",
           lambda: checked(_write(zstd_dir, good_zstd, {'samples': _mutate_sample(good_zstd['samples'], 0, 'wall_ns', -999)}), 'layout', 'smoke', 'mixed', 201, 0, 384))

    reject("boolean disguised as sample wall_ns is rejected",
           lambda: checked(_write(zstd_dir, good_zstd, {'samples': _mutate_sample(good_zstd['samples'], 0, 'wall_ns', True)}), 'layout', 'smoke', 'mixed', 201, 0, 384))

    reject("inflated match count diverging between full/projected/postings is rejected",
           lambda: checked(_write(zstd_dir, good_zstd, {'samples': _bump_matches(good_zstd['samples'], 3)}), 'layout', 'smoke', 'mixed', 201, 0, 384))

    with tempfile.TemporaryDirectory(prefix='cost-repair-review-anchor-') as tmp:
        tmp = Path(tmp)
        anchor_copy = tmp / 'table-anchor.json'
        anchor_copy.write_bytes((zstd_dir / 'table-anchor.json').read_bytes() + b' ')
        (tmp / 'table.parquet').write_bytes((zstd_dir / 'table.parquet').read_bytes())
        (tmp / 'postings.json').write_bytes((zstd_dir / 'postings.json').read_bytes())
        (tmp / 'postings-digest.txt').write_bytes((zstd_dir / 'postings-digest.txt').read_bytes())
        reject("mutated persisted table-anchor.json bytes are rejected against reported byte counts",
               lambda: validate_layout_artifacts(tmp, good_zstd))

    check("index_bytes equals index_data_bytes + index_digest_bytes (persisted, not resident-only)",
          good_zstd['index_bytes'] == good_zstd['index_data_bytes'] + good_zstd['index_digest_bytes'])
    check("table_anchor_bytes equals metadata_bytes for a non-JSON layout (anchor is durably counted)",
          good_zstd['table_anchor_bytes'] == good_zstd['metadata_bytes'] and good_zstd['table_anchor_bytes'] > 0)
    check("index_publication is a distinct measured phase from index_build",
          good_zstd.get('index_publication') is not None and good_zstd.get('index_build') is not None)

    with tempfile.TemporaryDirectory(prefix='cost-repair-review-summary-', dir=str(SMOKE.parent)) as tmp:
        tmp = Path(tmp)
        report = summarize(SMOKE, [('mixed', 201, 2048)], [-1, 0])
        cell = report['mixed-201-2048']['layouts']['zstd64']
        build = cell['postings_build_median_ns']
        publication = cell['postings_publication_median_ns']
        combined = cell['postings_build_plus_publication_median_ns']
        saving = cell['rare_postings_median_saving_ns']
        check("amortization combined field equals build + publication (not build alone)",
              combined == build + publication, f"build={build} publication={publication} combined={combined}")
        if saving is not None and saving > 0:
            import math
            expected_break_even = math.ceil(combined / saving)
            check("measured_break_even_queries uses ceil((build+publication)/saving)",
                  cell['measured_break_even_queries'] == expected_break_even,
                  f"got {cell['measured_break_even_queries']} want {expected_break_even}")

    if FAILURES:
        print(f"\n{len(FAILURES)} probe(s) failed:")
        for f in FAILURES:
            print(f" - {f}")
        return 1
    print("\nall probes passed")
    return 0


def _write(directory, base, overrides):
    data = copy.deepcopy(base)
    data.update(overrides)
    tmp = tempfile.NamedTemporaryFile(prefix='mutated-', suffix='.json', delete=False,
                                       dir=tempfile.gettempdir())
    Path(tmp.name).write_text(json.dumps(data))
    return Path(tmp.name)


def _dup_first_over_last(samples):
    samples = copy.deepcopy(samples)
    last_projected = [i for i, s in enumerate(samples) if s['mode'] == 'projected'][-1]
    first_projected = next(i for i, s in enumerate(samples) if s['mode'] == 'projected')
    samples[last_projected] = copy.deepcopy(samples[first_projected])
    return samples


def _mutate_sample(samples, index, field, value):
    samples = copy.deepcopy(samples)
    for s in samples:
        if s['mode'] == 'full' and s['index'] == index:
            s['timing'][field] = value
            break
    return samples


def _bump_matches(samples, index):
    samples = copy.deepcopy(samples)
    for s in samples:
        if s['index'] == index and s['mode'] == 'full':
            s['matches'] += 1
    return samples


if __name__ == '__main__':
    sys.exit(main())
