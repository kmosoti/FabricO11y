#!/usr/bin/env python3
"""Pinned SDK source; API assumptions require installed-source validation first.

Expected packages: opentelemetry-sdk==1.38.0 and
opentelemetry-exporter-otlp-proto-http==1.38.0. Assumptions: HTTP exporter
_export(serialized_data, timeout_sec=None) returns requests.Response and uses
uncompressed protobuf by default, with requests.Session in self._session and
post(..., data=body). SpanProcessor forwards export() on its worker.
The SDK, not this harness, creates spans and serializes ExportTraceServiceRequest.
"""
import argparse
import importlib.metadata
import json
import os
from pathlib import Path
import threading
import time
from urllib.parse import urlparse


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--endpoint', required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--rate', type=float, default=10)
    parser.add_argument('--duration', type=float, default=180)
    args = parser.parse_args()
    address = urlparse(args.endpoint)
    if address.scheme != 'http' or address.hostname not in ('127.0.0.1', '::1') or address.username or address.password or address.query or address.fragment:
        parser.error('endpoint must be numeric loopback HTTP without credentials/query/fragment')
    if not 0 < args.rate <= 1000 or not 0 < args.duration <= 1800:
        parser.error('rate must be in (0,1000], duration in (0,1800]')
    scratch = Path(os.environ['FABRIC_SCRATCH_ROOT']).resolve(strict=True)
    output = args.output.resolve()
    if not output.is_relative_to(scratch) or output == scratch or output.exists():
        parser.error('fresh output directory under FABRIC_SCRATCH_ROOT required')
    packages = {name: importlib.metadata.version(name) for name in
                ('opentelemetry-sdk', 'opentelemetry-exporter-otlp-proto-http')}
    if any(version != '1.38.0' for version in packages.values()):
        raise RuntimeError(f'pinned SDK versions required: {packages}')
    from opentelemetry import trace
    from opentelemetry.context import Context
    from opentelemetry.sdk.resources import Resource
    from opentelemetry.sdk.trace import TracerProvider
    from opentelemetry.sdk.trace.export import BatchSpanProcessor
    from opentelemetry.exporter.otlp.proto.http.trace_exporter import OTLPSpanExporter
    from opentelemetry.trace import Status, StatusCode

    output.mkdir(parents=True)
    attempts_path, spans_path = output / 'attempts.jsonl', output / 'source-spans.jsonl'
    lock = threading.Lock()

    def append(path, record):
        with lock, path.open('a') as stream:
            stream.write(json.dumps(record, separators=(',', ':')) + '\n')

    class RecordedExporter(OTLPSpanExporter):
        def __init__(self, **kwargs):
            self.sequence = 0
            super().__init__(**kwargs)

        def _export(self, serialized_data, timeout_sec=None):
            # Observe every post, including an SDK retry inside one _export.
            # The SDK retains serialization, session, compression and retry policy.
            previous_post = self._session.post

            def recorded_post(*positional, **keywords):
                self.sequence += 1
                number = self.sequence
                body = bytes(keywords['data'])
                if body != bytes(serialized_data):
                    raise RuntimeError('unexpected compression/body transformation; validate SDK API')
                path = output / f'attempt-{number:06d}.protobuf'
                path.write_bytes(body)
                receipt = {'attempt': number, 'body_file': path.name, 'body_bytes': len(body),
                           'start_wall_ns': time.time_ns(), 'start_mono_ns': time.monotonic_ns(),
                           'response_status': None, 'error': None}
                try:
                    response = previous_post(*positional, **keywords)
                    receipt['response_status'] = response.status_code
                    receipt['response_body_hex'] = response.content.hex()
                    return response
                except BaseException as error:
                    receipt['error'] = repr(error)
                    raise
                finally:
                    receipt['end_wall_ns'] = time.time_ns()
                    receipt['end_mono_ns'] = time.monotonic_ns()
                    append(attempts_path, receipt)

            self._session.post = recorded_post
            try:
                return super()._export(serialized_data, timeout_sec=timeout_sec)
            finally:
                self._session.post = previous_post

    provider = TracerProvider(resource=Resource.create({'service.name': 'fabric-sdk-completion',
                                                       'fabric.fixture': 'sdk-1.38.0'}))
    exporter = RecordedExporter(endpoint=args.endpoint, timeout=5)
    processor = BatchSpanProcessor(exporter, max_queue_size=4096, max_export_batch_size=64,
                                   schedule_delay_millis=1000, export_timeout_millis=5000)
    provider.add_span_processor(processor)
    tracer = provider.get_tracer('fabric-sdk-completion', '1')
    epoch_wall, epoch_mono = time.time_ns(), time.monotonic_ns()
    summary = {'packages': packages, 'endpoint': args.endpoint, 'rate_pairs_per_s': args.rate,
               'duration_s': args.duration, 'epoch_wall_ns': epoch_wall, 'epoch_mono_ns': epoch_mono,
               'queue': 4096, 'batch': 64, 'delay_ms': 1000, 'timeout_ms': 5000,
               'pairs': 0, 'spans': 0, 'force_flush': None, 'shutdown': None,
               'error': None, 'id_policy': 'SDK default random IDs, recorded exactly',
               'resource_attributes': dict(provider.resource.attributes),
               'api_validation': 'required before workload; see module assumptions'}
    try:
        index = 0
        while index / args.rate < args.duration:
            due = epoch_mono + int(index / args.rate * 1e9)
            remaining = (due - time.monotonic_ns()) / 1e9
            if remaining > 0:
                time.sleep(remaining)
            actual = time.monotonic_ns()
            start = epoch_wall + int(index / args.rate * 1e9)
            for child in (False, True):
                attributes = {'fabric.fixture.index': index, 'fabric.fixture.child': child,
                              'fabric.fixture.phase': int(index / args.rate // 60)}
                span = tracer.start_span('fixture.child' if child else 'fixture.parent',
                    context=trace.set_span_in_context(parent) if child else Context(),
                    start_time=start + (1000 if child else 0), attributes=attributes,
                    kind=trace.SpanKind.INTERNAL)
                if not child:
                    parent = span
                status = StatusCode.ERROR if index % 10 == 0 else StatusCode.OK
                span.set_status(Status(status, 'fixture-error' if status == StatusCode.ERROR else None))
                event_time = start + (2000 if child else 1500)
                span.add_event('fixture.event', {'ordinal': index}, timestamp=event_time)
                end = start + (3000 if child else 4000)
                context = span.get_span_context()
                parent_context = parent.get_span_context() if child else None
                span.end(end_time=end)
                append(spans_path, {'index': index, 'name': 'fixture.child' if child else 'fixture.parent',
                    'trace_id': f'{context.trace_id:032x}', 'span_id': f'{context.span_id:016x}',
                    'kind': 'INTERNAL', 'scope_name': 'fabric-sdk-completion', 'scope_version': '1',
                    'parent_span_id': f'{parent_context.span_id:016x}' if child else '',
                    'start_ns': start + (1000 if child else 0), 'end_ns': end,
                    'attributes': attributes, 'status': status.name,
                    'status_description': 'fixture-error' if status == StatusCode.ERROR else '',
                    'events': [{'name': 'fixture.event', 'timestamp_ns': event_time,
                                'attributes': {'ordinal': index}}],
                    'scheduled_mono_ns': due, 'producer_mono_ns': actual,
                    'lateness_ns': actual - due})
                summary['spans'] += 1
            summary['pairs'] += 1
            index += 1
    except BaseException as error:
        summary['error'] = repr(error)
        raise
    finally:
        try:
            summary['force_flush'] = provider.force_flush(timeout_millis=15000)
        except BaseException as error:
            summary['force_flush'] = {'error': repr(error)}
        try:
            provider.shutdown()
            summary['shutdown'] = 'returned'
        except BaseException as error:
            summary['shutdown'] = {'error': repr(error)}
        summary.update(end_wall_ns=time.time_ns(), end_mono_ns=time.monotonic_ns(),
                       http_attempts=exporter.sequence)
        (output / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')


if __name__ == '__main__':
    main()
