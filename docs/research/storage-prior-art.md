# Prior art: physical storage of logs, metrics and traces

Status: **research notes**, tier 6 of the [source-of-truth order](../README.md#source-of-truth). A web survey made on 2026-10-02 for the [storage direction](storage-direction.md). Every claim carries its source; items marked unverified could not be confirmed from a primary source. Nothing here is a decision.


## Summary

Production systems split into three families. (1) **Per-kind engines**: Prometheus/Mimir TSDB for metrics, Loki chunks for logs, Tempo Parquet blocks for traces, each with a layout tuned to one access pattern. (2) **One general columnar engine with per-kind tables**: ClickHouse deployments (SigNoz, ClickStack/HyperDX, Uptrace, Jaeger), Elastic (TSDS and LogsDB index modes), and Parquet-on-object-storage systems (InfluxDB 3, OpenObserve, Parseable, GreptimeDB) keep logs, traces and metrics in separate tables or streams but share one storage engine, codecs and query planner. (3) **One row model for everything**: Honeycomb and Meta's Scuba store "wide events" and derive traces from `trace_id`/`span_id` fields; Datadog Husky stores logs, traces, RUM and profiles as "structured logs" in one custom column store. Even in family (3), metrics are the exception: Honeycomb stores metrics in dedicated metrics datasets, and Monarch/Gorilla show why dense numeric streams want delta-of-delta and XOR encodings rather than a sparse event layout.

The encoding facts that recur: dictionary-encode low-cardinality strings; store attributes as typed columns (static, dedicated, or dynamically shredded) rather than JSON blobs when queries must be cheap; sort by a low-cardinality grouping key (tenant, service, resource fingerprint, `_tsid`) before time; keep `trace_id`/`span_id` as first-class columns in every kind; and for log bodies, separate the static template from variables (CLP, Drain, LogGrep).

## OpenTelemetry data model and OTel Arrow (OTAP)

- OTel LogRecord fields are Timestamp, ObservedTimestamp, TraceId, SpanId, TraceFlags, SeverityText, SeverityNumber, Body, Resource, InstrumentationScope, Attributes and EventName; Body and Attributes are `AnyValue`. Logs deliberately share Resource and W3C trace context with spans and metrics. https://opentelemetry.io/docs/specs/otel/logs/data-model/ (spec, current)
- Metric points are Sum, Gauge, Histogram, ExponentialHistogram and legacy Summary, with cumulative or delta temporality; **Exemplars** carry value, time, filtered attributes and optional `trace_id`/`span_id`. https://opentelemetry.io/docs/specs/otel/metrics/data-model/ (spec, current)
- Spans nest ResourceSpans > ScopeSpans > Span; `trace_id` is 16 bytes, `span_id` 8 bytes, with attributes, events, links and status. https://github.com/open-telemetry/opentelemetry-proto/blob/main/opentelemetry/proto/trace/v1/trace.proto (current)
- OTAP is a normalized multi-RecordBatch layout: a primary batch per signal plus separate attribute batches linked by `parent_id` (u16/u32), each attribute row having `key`, `type` and typed value columns (`str`, `int`, `double`, `bool`, `bytes`, `ser`). The model is explicitly designed for "compatibility with file formats, such as Parquet". https://github.com/open-telemetry/otel-arrow/blob/main/docs/data_model.md (current)
- OTEP 0156: low-cardinality strings use Arrow dictionary encoding with overflow to plain strings above about 2^16 values; rows are sorted to improve locality; `parent_id` uses delta group encoding. Benchmarks versus OTLP+zstd: univariate metrics 2-2.5x, multivariate 3-7x, logs 1.6-2x, traces 1.7-2.8x less bandwidth. https://github.com/open-telemetry/oteps/blob/main/text/0156-columnar-encoding.md (OTEP, undated page)
- Announcement blog: dictionary deltas are sent between consecutive batches on a gRPC stream; "40% improvement over the best existing OTLP configurations with zstd". https://opentelemetry.io/blog/2023/otel-arrow/ (2023). Production report at ServiceNow: traces compress 15x-30x of uncompressed, about 30% better than OTLP; logs and metrics 50-70% better; 500-600k spans/s. https://opentelemetry.io/blog/2024/otel-arrow-production/ (2024)

## Grafana Tempo (traces, Parquet)

- A block is `meta.json`, `data.parquet`, a bloom filter and an index mapping trace IDs to row groups; rows are traces "sorted by trace ID within the file", nested as resource spans > scope spans > spans. https://grafana.com/docs/tempo/latest/reference-tempo-architecture/block-format/ (docs, Tempo 3.1 era)
- The schema is nested rather than flat because "the block size is much smaller for the nested schema" (resource attributes stored once per resource). Generic attributes are stored as Key/IsArray/Value/ValueInt/ValueDouble/ValueBool/ValueUnsupported columns; timestamps use delta encoding; RLE works well on mostly-zero columns. https://grafana.com/docs/tempo/latest/operations/schema/ (docs, current)
- vParquet3 added up to 10 dedicated string attribute columns at span and at resource scope (dictionary encoded). In Grafana Cloud, generic attribute columns were "up to 70% of the overall block size" and the ten most used span attributes were "over 50% of the total attribute size"; dedicated columns cut tail latency up to 75%, CPU up to 70%, memory up to 50%. https://grafana.com/blog/accelerate-traceql-queries-at-scale-with-dedicated-attribute-columns-in-grafana-tempo/ (2024-01-23)
- vParquet4 (default in Tempo 2.6, Sept 2024) added columns for events, links and array attributes; vParquet5 (default in 3.1) raises dedicated columns to 20 strings and 5 integers per scope, adds blob columns with zstd and materialized timestamps for metrics queries; vParquet2 was removed in 3.0 and vParquet3 deprecated in 3.1. https://grafana.com/docs/tempo/latest/release-notes/v2-6/ , https://grafana.com/docs/tempo/latest/configuration/parquet/ (docs)

## Grafana Loki (logs) and Prometheus/Mimir (metrics)

- Loki: a chunk holds one stream's entries; header is magic(4b), version(1b), encoding(1b); blocks of `ts varint, len uvarint, bytes` are compressed, followed by a structured-metadata symbol section and a per-block checksum table. https://grafana.com/docs/loki/latest/get-started/architecture/ (docs, current). The TSDB index (recommended since 2.8) is derived from Prometheus TSDB, maps label sets to chunk references and records chunk size and line count for query planning. https://grafana.com/docs/loki/latest/operations/storage/tsdb/ (docs)
- Loki chunk tuning: default encoding gzip, recommended snappy; recommended `chunk_target_size` 1.5 MB compressed, which needs "5-10x raw log data to fill". https://grafana.com/docs/loki/latest/configure/bp-configure/ (docs)
- Prometheus XOR chunk: 16-bit sample count, first timestamp varint and float64, then delta-of-delta timestamps and XOR values with variable bit widths; 4-byte CRC32C per chunk. https://github.com/prometheus/prometheus/blob/main/tsdb/docs/format/chunks.md (current). Two-hour blocks with `chunks/`, `index`, `meta.json`, tombstones; "an average of only 1-2 bytes per sample". https://prometheus.io/docs/prometheus/latest/storage/ (docs)
- Mimir uploads per-tenant TSDB blocks (two-hour range, "around 120 samples per chunk") to object storage and compacts them to 12h/24h. https://grafana.com/docs/mimir/latest/get-started/about-grafana-mimir-architecture/ (docs)
- Exemplars in Prometheus live in "a fixed size circular buffer" in memory plus the WAL, not in blocks. https://prometheus.io/docs/prometheus/latest/feature_flags/ (docs)

## Gorilla (VLDB 2015) and Monarch (VLDB 2020)

- Gorilla: delta-of-delta timestamps and XOR floats "compress each by series down from 16 bytes to an average of 1.37 bytes, a 12x reduction"; two-hour blocks were chosen because longer blocks gave "diminishing returns". https://www.vldb.org/pvldb/vol8/p1816-teller.pdf (2015)
- Monarch: tables have a target schema and a metric schema; "target ranges are used for lexicographic sharding and load balancing among leaves"; the in-memory store does "only light compression such as timestamp sharing and delta encoding", with one timestamp sequence shared by about ten series; finalized points use delta and run-length encoding; distributions carry exemplars. https://www.vldb.org/pvldb/vol13/p3181-adams.pdf (2020)

## ClickHouse-based stores: SigNoz, ClickStack, Uptrace, Jaeger

- SigNoz logs (`logs_v2`): typed maps `attributes_string/number/bool`, a `resource` JSON column, `resource_fingerprint`, `ts_bucket_start` (30-minute bucket), `trace_id`/`span_id` columns, and materialized `attribute_<type>_<key>` columns for selected keys. https://signoz.io/docs/userguide/logs_clickhouse_queries/ (docs). ORDER BY `(ts_bucket_start, resource_fingerprint, severity_text, timestamp, id)` cut blocks scanned from 99.5% to 0.85% for a namespace filter. https://signoz.io/blog/query-performance-improvement (2025)
- SigNoz traces (`signoz_index_v3`): ORDER BY `(ts_bucket_start, resource_fingerprint, has_error, name, timestamp)`. https://signoz.io/docs/userguide/writing-clickhouse-traces-query/ (docs). Metrics: `samples_v4` with Delta+ZSTD fingerprints, DoubleDelta+ZSTD timestamps and Gorilla codec values, joined by fingerprint to `time_series_v4` tables holding labels as JSON strings. https://signoz.io/docs/userguide/write-a-metrics-clickhouse-query/ (docs)
- ClickStack/HyperDX: separate `otel_logs`, `otel_traces` and five `otel_metrics_*` tables; attributes as `Map(LowCardinality(String), String)`; ZSTD(1) plus Delta on timestamps; bloom and tokenbf skip indexes; partition by day; logs/traces ORDER BY `(ServiceName, Timestamp)`, metrics `(ServiceName, MetricName, Attributes, TimeUnix)`. https://clickhouse.com/docs/use-cases/observability/clickstack/ingesting-data/schemas (docs)
- Uptrace: the vendor page claims "10:1 compression ratios typical" and that "some attributes" are placed into separate columns; a `spans_index` / `spans_data` split (truncated index table plus full-data table) is described only in secondary search snippets and is **unverified** here. https://uptrace.dev/get/contributing.html , https://preview.uptrace.dev/features/querying/spans
- Jaeger v2.18 ClickHouse backend: ORDER BY `(service_name, name, start_time)` with a bloom filter on `trace_id`; attributes in Nested columns per level (resource, scope, span, event, link) keeping Bool/Int64/Float64/String types; 10M spans from about 6 GiB to 722 MiB (8.6x). https://www.cncf.io/blog/2026/06/23/building-jaegers-clickhouse-backend-8-6x-compression-on-10-million-spans/ (2026-06-23)
- ClickHouse JSON type: each path becomes a typed subcolumn; `max_dynamic_paths` (1024) and `max_dynamic_types` (32), overflow to a shared binary column. https://clickhouse.com/blog/a-new-powerful-json-data-type-for-clickhouse (2024-10-22). JSONBench, 1B documents: ClickHouse 99 GB, MongoDB 158 GB, Elasticsearch 220 GB without `_source`, DuckDB 472 GB, PostgreSQL 622 GB. https://clickhouse.com/blog/json-bench-clickhouse-vs-mongodb-elasticsearch-duckdb-postgresql (2025-01-29)

## Datadog Husky

- Third-generation event store for logs, traces, NPM, RUM and profiler events, all treated as "structured logs"; custom columnar file format on object storage with a FoundationDB-backed metadata store. https://www.datadoghq.com/blog/engineering/introducing-husky/ (2022-05-17)
- Fragments hold many columns, each with a header and fixed-size row groups; writers emit about 1,000-row fragments, size-tiered lazy compaction yields about 1M rows; fragments are bucketed by tenant and time window and rows are sorted by tags such as `service, status, env, timestamp`; "locality compaction" reduced query worker replicas by 30%. https://www.datadoghq.com/blog/engineering/husky-storage-compaction/ (2025-01-29)

## Honeycomb (Retriever) and Scuba

- Honeycomb: one file per column, segments closed with min/max timestamp metadata, no indexes, every query time-bounded. https://www.honeycomb.io/blog/why-observability-requires-distributed-column-store (date not shown on page; **unverified**). Segments are about 1 GB / 1M events / 12 h and are queried from S3 via Lambda; LZ4 replaced gzip for 3x faster reads. https://www.honeycomb.io/blog/secondary-storage-to-just-storage (2019-12-12)
- Traces and logs are events with `trace.trace_id`, `trace.span_id`, `trace.parent_id`; metrics are "time-series data points in dedicated metrics datasets rather than as events". https://docs.honeycomb.io/get-started/honeycomb/traces-metrics-logs.md (docs)
- Scuba: in-memory, 144 GB servers, schema inferred per node and reconciled at aggregation, rows carry a sample rate; dbdb.io describes the storage as row-based with dictionary and varint encoding, while a 2019-era summary says columns within row blocks with "dictionary encoding, bit packing, delta encoding, and lz4". https://dbdb.io/db/scuba , https://compileralchemy.substack.com/p/scuba-diving-into-the-extraordinary (paper: VLDB 2013)

## Elastic TSDS and LogsDB

- TSDS (`index.mode: time_series`): dimensions produce `_tsid`; segments are sorted by `_tsid` then `@timestamp`; synthetic `_source`; "70% less disk space than a regular data stream" in Elastic's benchmark. https://www.elastic.co/docs/manage-data/data-store/data-streams/time-series-data-stream-tsds (docs)
- LogsDB: default sort `host.name` asc, `@timestamp` desc; up to 65% smaller, of which index sorting gives up to 30% and synthetic `_source` 20-40%; zstd, delta and RLE codecs chosen automatically. https://www.elastic.co/search-labs/blog/elasticsearch-logsdb-index-mode (2024-12-12). `flattened` stores a whole object as keyword leaves to avoid mapping explosion; `index.mapping.total_fields.limit` defaults to 1000 with `ignore_dynamic_beyond_limit`. https://www.elastic.co/docs/reference/elasticsearch/mapping-reference/flattened , https://www.elastic.co/docs/reference/elasticsearch/index-settings/mapping-limit (docs)

## Quickwit, Parseable, OpenObserve, InfluxDB 3, GreptimeDB

- Quickwit splits hold an inverted index, columnar fast fields, a row doc store and a hotcache (<0.1% of split); 10M GitHub events gave a 15 GB split, "a compression ratio of almost 3"; splits carry min/max timestamps for pruning. https://quickwit.io/blog/quickwit-101 (date not captured; **unverified**)
- Parseable: Parquet on object storage, logs, metrics and traces over OTLP, "stateless compute over object storage". https://github.com/parseablehq/parseable/blob/main/README.md (undated)
- OpenObserve: Parquet on object storage, one memtable per `organization/stream_type` (logs, metrics, traces are separate streams), schema evolution under lock; README claims "140x lower storage cost" versus Elasticsearch (vendor claim). https://openobserve.ai/docs/architecture/ , https://github.com/openobserve/openobserve/blob/main/README.md
- InfluxDB 3: Parquet partitioned by day, ingester "picks the least cardinality columns for the sort order", files "often 10-100x smaller than its raw form", DataFusion/Arrow queries. https://www.influxdata.com/blog/influxdb-3-0-system-architecture/ (updated 2025-05-21)
- GreptimeDB positions one Parquet-based engine for wide events, logs, metrics and traces. https://greptime.com/blogs/2025-04-25-greptimedb-observability2-new-database (2025-04-25)

## Log compression papers: CLP, CLP-S, LogReducer, LogGrep, Drain

- CLP (OSDI 2021): messages split into a logtype (static text with variable placeholders), a variable dictionary and encoded variables, then compressed column-wise with Zstandard; average compression ratio 32.20 versus gzip 16.38, Splunk 2.86, Elasticsearch 1.75; 4.2x faster search than Splunk, 1.3x than Elasticsearch, 7.8x than ripgrep; ingestion 13x faster than Elasticsearch/Splunk. https://www.usenix.org/conference/osdi21/presentation/rodrigues , https://usenix.org/system/files/osdi21_slides_rodrigues.pdf (2021). Pinot's CLP columns are `_logtype`, `_dictionaryVars`, `_encodedVars`. https://docs.pinot.apache.org/build-with-pinot/ingestion/stream-ingestion/clp
- Uber: 5.38 PB of Spark INFO logs over 30 days compressed to 31.4 TB (169x) in the Log4j appender; phase 2 columnar aggregation expected another 2x. https://www.uber.com/blog/reducing-logging-cost-by-two-orders-of-magnitude-using-clp/ (2022-09-29)
- μSlope / CLP-S (OSDI 2024): JSON logs, schema stored once per dataset, records with the same schema consolidated into tables; 21.9:1 to 186.8:1, 2.34x better than Zstandard. https://www.usenix.org/conference/osdi24/presentation/wang-rui (2024)
- LogReducer (FAST 2021): numerical values dominate residual size; delta timestamps, correlation identification, elastic encoding; highest ratio on 18 production and 16 public log types. https://www.usenix.org/conference/fast21/presentation/wei (2021)
- LogGrep (EuroSys 2023): static plus runtime patterns; Hadoop 16.8 GB to 331 MB, Thunderbird 31 GB to 622 MB; 13.74x faster queries than CLP at 41% of its cost. https://github.com/THUBear-wjy/LogGrep/blob/main/README.md (2023)
- Drain (ICWS 2017): online template extraction with a fixed-depth parse tree keyed on token count and leading tokens. https://jiemingzhu.github.io/pub/pjhe_icws2017.pdf (2017)

## Columnar format references: Procella, Lance, Iceberg/Delta, Parquet Variant

- Procella's Artus format stores "custom encoding schemes rather than ... LZW", picks encodings per column by sampling, stores nested data as a tree of fields each a column, keeps zone maps, bloom filters and inverted indices in the metadata store, and evaluates on encoded data; one dataset was 2,060 KB in Artus versus 2,406 KB in Capacitor. https://research.google/pubs/procella-unifying-serving-and-analytical-data-at-youtube/ (VLDB 2019), https://tech.marksblogg.com/youtube-database-procella.html
- Lance v2 removes row groups, stores pages per column with column metadata at file end and pluggable per-column encodings, for random access. https://lancedb.com/blog/lance-v2 (undated)
- Iceberg manifests record per-file `lower_bounds`, `upper_bounds`, `null_value_counts`, `value_counts`, `column_sizes`; sort orders and partition transforms `year/month/day/hour`, `bucket[N]`, `truncate[W]`. https://raw.githubusercontent.com/apache/iceberg/main/format/spec.md. Delta Lake collects min/max for the first N columns (`delta.dataSkippingNumIndexedCols`) and offers Z-ordering. https://docs.delta.io/latest/optimizations-oss.html
- Parquet Variant shredding: each value has `value` (binary variant) and `typed_value` (native type); objects can be partially shredded; typed statistics allow page skipping. https://github.com/apache/parquet-format/blob/master/VariantShredding.md

## The "wide events" argument and its critics

- Charity Majors: observability 1.0 means "paying to store telemetry five different ways ... for every single request"; custom-metric bills grow linearly with metric count; 2.0 uses "arbitrarily-wide structured log events" as one source of truth. https://www.honeycomb.io/blog/cost-crisis-observability-tooling (updated 2024-12-18)
- Ivan Burmistrov (ex-Meta): "Traces, Metrics and Logs are all just special cases of Wide Events"; Scuba's sampling makes cardinality a non-issue. https://isburmistrov.substack.com/p/all-you-need-is-wide-events-not-metrics (2024-02-15)
- Critique (Laban Eilers, SimpliSafe): 1,000 req/s as wide events about $33,000/month versus $0.80/month as adaptive metrics; sampling creates forensic blind spots; 2.0 is "a north star, not a silver bullet". https://grafana.com/events/obsessions/2025/boston/are-we-ready-for-observability-2-0-pragmatic-observability-at-simplisafe/ (2025-04-17)
- Meta "Nectar": no public primary source found; **unverified**.

## Comparison table

| System | One engine for all three? | Row model | Attribute encoding | Sort key | Log-body encoding | Reported compression |
|---|---|---|---|---|---|---|
| OTAP (wire) | One format, per-signal batches | Normalized batches + attribute tables | Dictionary + typed value columns, `parent_id` | Sorted for locality | Body as AnyValue column | 1.6-2x (logs) to 3-7x (multivariate metrics) over OTLP+zstd |
| Tempo | Traces only | One row per trace, nested | Key/typed-value lists + up to 20 dedicated cols | Trace ID | n/a | dedicated cols: -75% tail latency; no ratio given |
| Loki + TSDB | Logs only | Stream chunks | Labels in index; structured metadata symbols | Stream, time | Raw lines, gzip/snappy/lz4 | 5-10x needed to fill 1.5 MB chunk |
| Prometheus/Mimir | Metrics only | Series chunks | Labels in block index | Series, time | n/a | 1-2 bytes/sample |
| Gorilla | Metrics only | In-memory series | n/a | Series, time | n/a | 1.37 bytes/point, 12x |
| Monarch | Metrics only | Schematized target+metric tables | Key columns in target schema | Lexicographic by target | n/a | timestamp sharing ~10 series/sequence |
| SigNoz (ClickHouse) | One engine, per-kind tables | Row per log/span/sample | Typed Maps + JSON resource + materialized cols | `ts_bucket, resource_fingerprint, ...` | Body string + tokenbf | blocks scanned 99.5% -> 0.85% |
| ClickStack | One engine, per-kind tables | Row per record | Map(LowCardinality, String) | `ServiceName, Timestamp` | Body + tokenbf | ZSTD(1); no ratio given |
| Jaeger/ClickHouse | Traces (OTel model) | Row per span | Nested typed columns per level | `service, name, start_time` | n/a | 8.6x |
| Husky | One engine; metrics separate (unverified) | Event rows | Schemaless per-column | tenant/time bucket, then tags, time | event columns | not published |
| Honeycomb | Events; metrics datasets separate | Wide events | One file per field | Time segments | fields | LZ4; no ratio |
| Scuba | Events | Wide events, sampled | Per-column dict/bitpack/delta/lz4 | Time | fields | not published |
| Elastic TSDS/LogsDB | One engine, index modes | Documents | Doc values, flattened, dynamic mappings | `_tsid`/`host.name`, `@timestamp` | text + synthetic source | -70% (TSDS), -65% (LogsDB) |
| Quickwit | Logs, traces (Jaeger) | Documents in splits | fast fields + doc store | time-pruned splits | inverted index | ~3x |
| InfluxDB 3 | Metrics (table model) | Rows in Parquet | columns | least-cardinality tags, time | n/a | 10-100x vs raw |
| OpenObserve / Parseable | One engine, per-kind streams | Rows in Parquet | evolving schema | time partitions | columns | vendor claims only |
| CLP / CLP-S | Logs | logtype + vars table | dictionaries | archive/segment | template + dict + encoded vars | 32x avg; 169x at Uber; 21.9-186.8x JSON |

## Design facts that transfer

1. OTAP already defines a columnar, Parquet-compatible layout for all three signals: a main batch per signal plus attribute tables keyed by `parent_id`, with dictionary encoding and typed value columns. (OTEP 0156; otel-arrow data_model.md)
2. Dictionary encoding of attribute keys and low-cardinality values is the single most repeated technique (OTAP, Tempo dedicated columns, ClickHouse LowCardinality, Scuba, CLP).
3. Promoting the most-used attributes to dedicated typed columns pays: Tempo measured generic attribute columns at up to 70% of block size and the top ten span attributes at over 50% of attribute bytes. (Grafana, 2024-01-23)
4. Dynamic typed subcolumns with an overflow bucket are the current answer to arbitrary attribute sets: ClickHouse JSON (`max_dynamic_paths` 1024, shared data column), Parquet Variant shredding (`typed_value` + binary `value`), Elastic `flattened` plus field limits.
5. Sorting by a low-cardinality grouping key before time is reported to matter more than codec choice: SigNoz resource fingerprint (99.5% -> 0.85% blocks), Elastic `_tsid`/`host.name` (up to 30% from sorting alone), InfluxDB 3 least-cardinality-first, Husky `service, status, env, timestamp`, Jaeger `service_name, name, start_time`.
6. Trace-ID-first sort is used only where the workload is "fetch one trace" (Tempo); search-first systems sort by service and keep a bloom filter on `trace_id` instead (Jaeger/ClickHouse, Tempo's per-block bloom).
7. Time bucketing is layered on top of the sort key, not replaced by it: SigNoz `ts_bucket_start` (30 min), ClickStack day partitions, Prometheus/Mimir 2-hour blocks, Husky tenant/time buckets, Quickwit and Honeycomb segment min/max timestamps, Iceberg/Delta per-file min/max.
8. Dense numeric series want delta-of-delta timestamps and XOR/Gorilla values: 1.37 bytes/point (Gorilla), 1-2 bytes/sample (Prometheus), and SigNoz applies the same codecs inside ClickHouse.
9. Monarch's timestamp sharing (one timestamp sequence per ~10 series from the same target) is a cheap encoding for metrics that share a scrape; OTAP's multivariate-metrics gain (3-7x) is the same observation at the wire.
10. Log bodies compress best when the static template is separated from variables: CLP 32x average versus gzip 16x, Uber 169x, μSlope 2.34x over zstd on JSON, LogReducer showing numeric fields dominate the residual.
11. Nested resource > scope > record storage avoids repeating resource attributes per record (Tempo schema rationale, OTLP proto shape).
12. Metrics remain the exception in "one row model" systems: Honeycomb stores metrics in dedicated datasets; Husky's published posts cover events, not metrics; the cost critique quotes $33,000/month versus $0.80/month for the same signal as wide events versus metrics.
13. Cross-kind joins in practice are plain columns: `trace_id`/`span_id` on logs and spans in every schema surveyed, and exemplars carrying `trace_id` on metric points (OTel) with Prometheus holding exemplars only in memory and WAL.
14. Reported chunk and file sizing: Loki 1.5 MB compressed chunks, Honeycomb about 1 GB / 1M events, Husky about 1M rows after compaction, Prometheus about 120 samples per chunk, Quickwit splits of about 10M docs.
15. Vendor ratios (OpenObserve 140x, Uptrace 10:1) are unaccompanied by workload definitions and should not be used as design inputs without a registered workload.
