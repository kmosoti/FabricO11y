"""Registered comparison: Fabric against Elasticsearch, ClickHouse and a commercial log platform.

Protocol: docs/experiments/benchmarks/baseline-comparison-protocol.md.
Writes engine data under a fresh target/alpha-* directory given by --out. The fixture is a
finished Segment-mode history trial directory (its server state, config,
certificates and simulator summary). The commercial platform's adapter and
unredacted results live outside the repository.
"""

import argparse
import base64
import hashlib
import importlib.util
import json
import os
import random
import shutil
import signal
import socket
import ssl
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import query_oracle  # noqa: E402
from workload import entropy_body  # noqa: E402

REPEATS = 20
SERVER_CPUS = "0-3"
CHILDREN = []


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def percentile(values, q):
    if not values:
        return None
    ordered = sorted(values)
    return ordered[min(len(ordered) - 1, int(q * len(ordered)))]


def tree_rss_kib(pids):
    total = 0
    for pid in pids:
        try:
            for line in Path(f"/proc/{pid}/status").read_text().splitlines():
                if line.startswith("VmRSS:"):
                    total += int(line.split()[1])
        except OSError:
            pass
    return total


def descendants(pid):
    found = {pid}
    changed = True
    while changed:
        changed = False
        for entry in Path("/proc").iterdir():
            if not entry.name.isdigit():
                continue
            try:
                ppid = int((entry / "stat").read_text().rpartition(") ")[2].split()[1])
            except OSError:
                continue
            if ppid in found and int(entry.name) not in found:
                found.add(int(entry.name))
                changed = True
    return found


def dir_bytes(path):
    path = Path(path)
    return sum(p.stat().st_size for p in path.rglob("*") if p.is_file()) if path.exists() else 0


def http(method, url, body=None, headers=None, ctx=None, timeout=600):
    data = body if body is None or isinstance(body, bytes) else body.encode()
    request = urllib.request.Request(url, data=data, method=method, headers=headers or {})
    try:
        with urllib.request.urlopen(request, context=ctx, timeout=timeout) as response:
            return response.status, response.read()
    except urllib.error.HTTPError as error:
        detail = error.read()[:600].decode(errors="replace")
        raise urllib.error.HTTPError(error.url, error.code, f"{error.reason}: {detail}", error.headers, None)


def wait_http(url, headers=None, ctx=None, limit=180):
    deadline = time.monotonic() + limit
    while time.monotonic() < deadline:
        try:
            http("GET", url, headers=headers, ctx=ctx, timeout=5)
            return
        except Exception:
            time.sleep(0.5)
    raise RuntimeError(f"{url} did not answer")


# ---------------------------------------------------------------- queries


def make_queries(seed, t0, span, nodes):
    rng = random.Random(seed ^ 0xC0FFEE)
    out = []

    def window():
        a = t0 + rng.randrange(0, span - 60 * 10**9)
        return a, a + 60 * 10**9

    for _ in range(REPEATS):
        a, b = window()
        out.append(("host_logs", {"kind": "logs", "node": f"sim{rng.randrange(nodes):04d}",
                                  "from_ns": a, "to_ns": b, "limit": 1000}))
        a, b = window()
        tick = ((a - t0) // 10**9 + 10) * 2 + 1
        needle = entropy_body(seed, rng.randrange(nodes), tick)[100:112]
        out.append(("text_search", {"kind": "logs", "from_ns": a, "to_ns": b, "contains": needle, "limit": 100}))
        out.append(("metric_history", {"kind": "metrics", "node": f"sim{rng.randrange(nodes):04d}",
                                       "name": "sim.metric.7", "from_ns": t0, "to_ns": t0 + span, "limit": 1000}))
        a, b = window()
        out.append(("fleet_metrics", {"kind": "metrics", "name": "sim.metric.3", "from_ns": a, "to_ns": b,
                                      "limit": 10000}))
    rng.shuffle(out)
    return out


def identity(row):
    return (row["node_id"], int(row["sequence"]), int(row["index"]))


def content(row, kind):
    if kind == "logs":
        return (int(row["observed_ns"]), row["node"], row["body"])
    return (int(row["time_ns"]), row["node"], row["name"], row["value"])


def exact(expected, got, kind):
    return ([identity(r) for r in expected] == [identity(r) for r in got]
            and [content(r, kind) for r in expected] == [content(r, kind) for r in got])


# ---------------------------------------------------------------- systems


class Fabric:
    name = "fabric"

    def __init__(self, fixture, bins, root):
        self.state = root / "fabric-state"
        shutil.copytree(fixture / "server-state", self.state)
        self.port = free_port()
        self.conf = root / "fabric-server.conf"
        text = (fixture / "server.conf").read_text().splitlines()
        text = [l for l in text if not l.startswith(("listen=", "state_dir="))]
        self.conf.write_text("\n".join(text + [f"listen=127.0.0.1:{self.port}", f"state_dir={self.state}"]) + "\n")
        self.ctx = ssl.create_default_context(cafile=str(fixture / "ca.pem"))
        self.auth = {"authorization": f"Bearer {(fixture / 'admin-token').read_text().strip()}",
                     "content-type": "application/json"}
        self.proc = subprocess.Popen(["taskset", "-c", SERVER_CPUS, str(bins / "fabric-server"), "serve",
                                      str(self.conf)], stdout=open(root / "fabric.log", "wb"),
                                     stderr=subprocess.STDOUT)
        CHILDREN.append(self.proc)
        wait_http(f"https://127.0.0.1:{self.port}/v1/admin/nodes", self.auth, self.ctx)

    def load(self, logs, metrics):
        return 0.0  # loaded through its delivery path by the fixture run

    def pids(self):
        return descendants(self.proc.pid)

    def bytes_at_rest(self):
        return dir_bytes(self.state)

    def query(self, q):
        # The first page: up to `limit` rows, like every baseline's one request.
        _, raw = http("POST", f"https://127.0.0.1:{self.port}/v1/admin/query", json.dumps(q), self.auth, self.ctx)
        return json.loads(raw)["rows"]

    def stop(self):
        self.proc.send_signal(signal.SIGTERM)
        self.proc.wait(timeout=120)


class Elastic:
    name = "elasticsearch"

    def __init__(self, home, root):
        self.port = free_port()
        conf = root / "es-config"
        shutil.copytree(home / "config", conf)
        (conf / "elasticsearch.yml").write_text(
            f"cluster.name: fabric-baseline\nnode.name: es1\npath.data: {root}/es-data\npath.logs: {root}/es-logs\n"
            f"network.host: 127.0.0.1\nhttp.port: {self.port}\ntransport.port: {free_port()}\n"
            "discovery.type: single-node\nxpack.security.enabled: false\nxpack.ml.enabled: false\n")
        env = dict(os.environ, ES_PATH_CONF=str(conf))
        self.data = root / "es-data"
        self.proc = subprocess.Popen(["taskset", "-c", SERVER_CPUS, str(home / "bin" / "elasticsearch")],
                                     env=env, stdout=open(root / "es.log", "wb"), stderr=subprocess.STDOUT)
        CHILDREN.append(self.proc)
        self.base = f"http://127.0.0.1:{self.port}"
        wait_http(self.base, limit=300)
        common = {"node": {"type": "keyword"}, "node_id": {"type": "keyword"}, "sequence": {"type": "long"},
                  "index": {"type": "integer"}, "attributes": {"type": "flattened"}}
        mappings = {
            "fabric-logs": {**common, "observed_ns": {"type": "long"}, "body": {"type": "wildcard"}},
            "fabric-metrics": {**common, "name": {"type": "keyword"}, "unit": {"type": "keyword"},
                               "kind": {"type": "keyword"}, "time_ns": {"type": "long"},
                               "start_ns": {"type": "long"}, "value_int": {"type": "long"},
                               "value_double": {"type": "double"}},
        }
        for index, props in mappings.items():
            http("PUT", f"{self.base}/{index}", json.dumps({
                "settings": {"number_of_shards": 1, "number_of_replicas": 0},
                "mappings": {"dynamic": "strict", "properties": props}}), {"content-type": "application/json"})

    def load(self, logs, metrics):
        started = time.monotonic()
        for index, rows in (("fabric-logs", logs), ("fabric-metrics", metrics)):
            for i in range(0, len(rows), 5000):
                lines = []
                for r in rows[i:i + 5000]:
                    lines.append('{"index":{}}')
                    lines.append(json.dumps(r))
                _, raw = http("POST", f"{self.base}/{index}/_bulk", "\n".join(lines) + "\n",
                              {"content-type": "application/x-ndjson"})
                if json.loads(raw).get("errors"):
                    raise RuntimeError(f"bulk errors in {index}")
        http("POST", f"{self.base}/_refresh")
        for index, rows in (("fabric-logs", logs), ("fabric-metrics", metrics)):
            count = json.loads(http("GET", f"{self.base}/{index}/_count")[1])["count"]
            if count != len(rows):
                raise RuntimeError(f"{index} holds {count}, expected {len(rows)}")
        return time.monotonic() - started

    def pids(self):
        return descendants(self.proc.pid)

    def bytes_at_rest(self):
        http("POST", f"{self.base}/_flush")
        return dir_bytes(self.data)

    def query(self, q):
        time_field = "observed_ns" if q["kind"] == "logs" else "time_ns"
        filters = [{"range": {time_field: {"gte": q["from_ns"], "lt": q["to_ns"]}}}]
        if q.get("node"):
            filters.append({"term": {"node": q["node"]}})
        if q.get("contains"):
            needle = q["contains"].replace("\\", "\\\\").replace("*", "\\*").replace("?", "\\?")
            filters.append({"wildcard": {"body": {"value": f"*{needle}*"}}})
        if q["kind"] == "metrics":
            filters.append({"term": {"name": q["name"]}})
        index = "fabric-logs" if q["kind"] == "logs" else "fabric-metrics"
        body = {"query": {"bool": {"filter": filters}}, "size": q["limit"],
                "sort": [{time_field: "asc"}, {"node_id": "asc"}, {"sequence": "asc"}, {"index": "asc"}]}
        _, raw = http("POST", f"{self.base}/{index}/_search", json.dumps(body), {"content-type": "application/json"})
        return [h["_source"] for h in json.loads(raw)["hits"]["hits"]]

    def stop(self):
        self.proc.send_signal(signal.SIGTERM)
        self.proc.wait(timeout=120)


class ClickHouse:
    name = "clickhouse"

    def __init__(self, binary, root):
        self.port = free_port()
        self.data = root / "ch-data"
        self.data.mkdir()
        self.proc = subprocess.Popen(
            ["taskset", "-c", SERVER_CPUS, str(binary), "server", "--",
             f"--path={self.data}/", f"--http_port={self.port}", f"--tcp_port={free_port()}",
             "--listen_host=127.0.0.1", f"--logger.log={root}/ch.log", f"--logger.errorlog={root}/ch.err.log",
             f"--tmp_path={self.data}/tmp/", f"--user_files_path={self.data}/user_files/"],
            cwd=self.data, stdout=open(root / "ch.stdout", "wb"), stderr=subprocess.STDOUT)
        CHILDREN.append(self.proc)
        self.base = f"http://127.0.0.1:{self.port}/"
        wait_http(self.base + "ping")
        self.sql("CREATE TABLE logs (node String, node_id String, sequence UInt64, `index` UInt32, "
                 "observed_ns Int64, body String, attributes Map(String, String)) "
                 "ENGINE = MergeTree ORDER BY (node, observed_ns)")
        self.sql("CREATE TABLE metrics (node String, node_id String, sequence UInt64, `index` UInt32, "
                 "name String, unit String, kind String, time_ns Int64, start_ns Int64, "
                 "value_int Nullable(Int64), value_double Nullable(Float64), attributes Map(String, String)) "
                 "ENGINE = MergeTree ORDER BY (name, node, time_ns)")

    def sql(self, text, body=None):
        url = self.base + "?" + urllib.parse.urlencode({
            "query": text, "output_format_json_quote_64bit_integers": 0, "date_time_input_format": "best_effort"})
        return http("POST", url, body)[1]

    def load(self, logs, metrics):
        started = time.monotonic()
        for table, rows in (("logs", logs), ("metrics", metrics)):
            for i in range(0, len(rows), 50000):
                payload = "\n".join(json.dumps(r) for r in rows[i:i + 50000])
                self.sql(f"INSERT INTO {table} FORMAT JSONEachRow", payload)
        for table, rows in (("logs", logs), ("metrics", metrics)):
            count = int(self.sql(f"SELECT count() FROM {table}").strip())
            if count != len(rows):
                raise RuntimeError(f"{table} holds {count}, expected {len(rows)}")
        return time.monotonic() - started

    def pids(self):
        return descendants(self.proc.pid)

    def bytes_at_rest(self):
        return dir_bytes(self.data)

    @staticmethod
    def lit(s):
        return "'" + s.replace("\\", "\\\\").replace("'", "\\'") + "'"

    def query(self, q):
        tf = "observed_ns" if q["kind"] == "logs" else "time_ns"
        where = [f"{tf} >= {q['from_ns']}", f"{tf} < {q['to_ns']}"]
        if q.get("node"):
            where.append(f"node = {self.lit(q['node'])}")
        if q.get("contains"):
            where.append(f"position(body, {self.lit(q['contains'])}) > 0")
        if q["kind"] == "metrics":
            where.append(f"name = {self.lit(q['name'])}")
        table = "logs" if q["kind"] == "logs" else "metrics"
        raw = self.sql(f"SELECT * FROM {table} WHERE {' AND '.join(where)} "
                       f"ORDER BY {tf}, node_id, sequence, `index` LIMIT {q['limit']} FORMAT JSONEachRow")
        return [json.loads(line) for line in raw.decode().splitlines() if line]

    def stop(self):
        self.proc.send_signal(signal.SIGTERM)
        self.proc.wait(timeout=120)


# ---------------------------------------------------------------- main


def main():
    try:
        return trial()
    finally:
        for child in CHILDREN:
            if child.poll() is None:
                child.kill()
                child.wait(timeout=30)


def trial():
    parser = argparse.ArgumentParser()
    parser.add_argument("--fixture", required=True)
    parser.add_argument("--bin-dir", required=True)
    parser.add_argument("--baselines", required=True)
    parser.add_argument("--systems", default="fabric,elasticsearch,clickhouse")
    parser.add_argument("--private-out", required=True, help="unredacted results for the commercial platform")
    parser.add_argument("--private-adapter", help="adapter module for the commercial platform, outside the repository")
    parser.add_argument("--seed", type=lambda v: int(v, 0), default=0xA11FA001)
    parser.add_argument("--limit-rows", type=int, default=0, help="smoke tests only")
    parser.add_argument("--out", required=True, help="fresh target/alpha-* directory for engine data")
    args = parser.parse_args()
    # Not under runner.py: ClickHouse keeps symlinks in its data directory,
    # which the runner's owned-tree scan refuses by design.
    root = Path(args.out).resolve()
    if root.parent.name != "target" or not root.name.startswith("alpha-") or root.exists():
        raise SystemExit("--out must be a new target/alpha-* directory")
    root.mkdir()
    fixture, bins, base = Path(args.fixture).resolve(), Path(args.bin_dir).resolve(), Path(args.baselines).resolve()
    private = Path(args.private_out).resolve()
    private.mkdir(parents=True, exist_ok=True)

    dump = subprocess.run([str(bins / "examples" / "server_dump"), str(fixture / "server.conf"), "--records"],
                          capture_output=True, text=True, timeout=1800, check=True)
    records = [json.loads(line) for line in dump.stdout.splitlines() if line]
    del dump
    mats = query_oracle.materialize_all(records)
    del records
    logs, metrics = [], []
    for m in mats:
        logs.extend(m.log_rows)
        for p in m.metric_points:
            row = {k: v for k, v in p.items() if k != "value"}
            row["value_int"] = p["value"] if isinstance(p["value"], int) else None
            row["value_double"] = p["value"] if isinstance(p["value"], float) else None
            metrics.append(row)
    if args.limit_rows:
        logs, metrics = logs[:args.limit_rows], metrics[:args.limit_rows]
        keep_logs = {identity(r) for r in logs}
        keep_metrics = {identity(r) for r in metrics}
        for m in mats:
            m.log_rows = [r for r in m.log_rows if identity(r) in keep_logs]
            m.metric_points = [r for r in m.metric_points if identity(r) in keep_metrics]
    sim = json.loads((fixture / "sim" / "sim-summary.json").read_text())
    queries = make_queries(args.seed, sim["began_unix_ns"], sim["seconds"] * 10**9, sim["identities"])
    expected = []
    for kind, q in queries:
        rows = (query_oracle._expected_log_rows(mats, q) if q["kind"] == "logs"
                else query_oracle._expected_metric_rows(mats, q))[:q["limit"]]
        expected.append(rows)
    del mats

    def as_answer(row, kind):
        if kind == "metrics" and "value_int" in row:
            row = dict(row)
            vi, vd = row.pop("value_int"), row.pop("value_double")
            row["value"] = vi if vi is not None else vd
        return row

    results = {}
    for system in args.systems.split(","):
        if system == "fabric":
            engine = Fabric(fixture, bins, root)
        elif system == "elasticsearch":
            engine = Elastic(base / "opt" / "elasticsearch-9.5.4", root)
        elif system == "clickhouse":
            engine = ClickHouse(next((base / "opt" / "clickhouse").glob("*/usr/bin/clickhouse")), root)
        elif system == "commercial":
            # A commercial platform whose license restricts publishing named
            # benchmark results. Its adapter lives outside this repository.
            spec = importlib.util.spec_from_file_location("commercial_adapter", args.private_adapter)
            module = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(module)
            engine = module.Adapter(base, root)
        else:
            raise SystemExit(f"unknown system {system}")
        try:
            load_s = engine.load(logs, metrics)
            peak = 0
            latency, exact_count = {}, {}
            for (kind, q), want in zip(queries, expected):
                started = time.monotonic()
                got = [as_answer(r, q["kind"]) for r in engine.query(q)]
                latency.setdefault(kind, []).append(time.monotonic() - started)
                exact_count[kind] = exact_count.get(kind, 0) + int(exact(want, got, q["kind"]))
                peak = max(peak, tree_rss_kib(engine.pids()))
            results[system] = {
                "load_s": round(load_s, 2), "bytes_at_rest": engine.bytes_at_rest(), "peak_rss_kib": peak,
                "queries": {k: {"n": len(v), "p50_s": percentile(v, 0.5), "p99_s": percentile(v, 0.99),
                                "exact": exact_count[k]} for k, v in latency.items()},
            }
        finally:
            engine.stop()
        print(json.dumps({system: results[system]}), flush=True)
    rows = {"logs": len(logs), "metrics": len(metrics)}
    # Published under a generic category, without the product's name or version.
    public = {("commercial log platform" if k == "commercial" else k): v for k, v in results.items()}
    (root / "comparison-summary.json").write_text(json.dumps({"rows": rows, "results": public}, sort_keys=True) + "\n")
    if "commercial" in results:
        (private / "commercial-comparison.json").write_text(json.dumps({"rows": rows, "commercial": results["commercial"]},
                                                                       sort_keys=True) + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
