import base64
import hashlib
import json
from pathlib import Path
import sys
import unittest
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
import populations
import producer


class EdgeLogPopulation(unittest.TestCase):
    def setUp(self):
        path = Path(__file__).resolve().parents[3] / 'docs/experiments/formal/fixtures/cross-edge-diagnostics.json'
        self.fixture = json.loads(path.read_text())
        self.row = {'attributes': dict(self.fixture['diagnostic_example']['attributes'])}
        self.acceptance = {'boot_id': 'edge-boot', 'lower_ns': 110, 'upper_ns': 112}

    def classify(self, row, offers=None, source=None):
        return populations.edge_log_start(row, source or self.fixture['offered_path'],
                                          offers or {}, self.acceptance, 'edge-boot')

    def test_retained_native_diagnostic_uses_its_acceptance_boundary(self):
        start, population = self.classify(self.row)
        self.assertIs(start, self.acceptance)
        self.assertEqual(population, self.fixture['expected_population'])

    def test_unknown_missing_and_similar_paths_remain_refused(self):
        for path in (None, '/var/log/other.log', self.fixture['diagnostic_path'] + '.1',
                     self.fixture['diagnostic_path'] + '/extra'):
            row = {'attributes': {'log.file.path': path}}
            with self.subTest(path=path), self.assertRaisesRegex(ValueError, 'unexpected unregistered'):
                self.classify(row)

    def test_offered_body_and_offset_join_remain_required(self):
        row = {'attributes': {'log.file.path': self.fixture['offered_path'],
                              'log.file.offset.start': '17'}, 'body': 'original'}
        offers = {17: {'body': 'original', 'before_ns': 10, 'after_ns': 12}}
        start, population = self.classify(row, offers)
        self.assertEqual(population, 'offered_edge')
        self.assertEqual((start['lower_ns'], start['upper_ns']), (10, 12))
        with self.assertRaisesRegex(ValueError, 'exact offered log bytes changed'):
            self.classify({**row, 'body': 'changed'}, offers)
        with self.assertRaises(KeyError):
            self.classify(row, {18: offers[17]})

    def test_diagnostic_path_cannot_replace_the_registered_offered_path(self):
        with self.assertRaisesRegex(ValueError, 'unregistered offered edge source'):
            self.classify(self.row, source=self.fixture['diagnostic_path'])


class NativePopulation(unittest.TestCase):
    def fixture(self):
        raw = producer.batch('a'*32, 1, producer.metrics(1, 10, 20))
        source = {'type':'source','node_id':'a'*32,'generation':1,'sequence':1,
                  'bytes':base64.b64encode(raw).decode()}
        state = {'type':'node_state','node_id':'a'*32,'generation':1,'retained_sequences':[1],'ack_cursor':1}
        def event(stage, at):
            return ('timing process_id=17 node_id='+'a'*32+' generation=1 sequence=1 stage='+stage+
                    f' unix_ns=1000 boot_monotonic_ns={at} monotonic_before_ns=10 monotonic_after_ns=12')
        log='\n'.join(event(stage, at) for stage,at in [('collection_started',100),('sources_accepted',110),
                      ('spool_committed',120),('send_started',125),('answer_received',150)])
        log+='\ndelivery sequence=1 sha256='+hashlib.sha256(raw).hexdigest()+' status=ack committed_through=1 elapsed_us=25\n'
        return [source,state],log

    def test_independently_retained_source_joins_actual_ack_bracket(self):
        sources,log=self.fixture()
        value=populations.native(sources,log,'boot')[1]
        self.assertEqual(value['collection']['lower_ns'],98)
        self.assertEqual(value['ack']['upper_ns'],152)
        self.assertEqual(value['attempts'],1)

    def test_changed_attempt_bytes_and_missing_attempt_clock_reject(self):
        sources,log=self.fixture()
        checksum=hashlib.sha256(base64.b64decode(sources[0]['bytes'])).hexdigest()
        for altered in [log.replace(checksum,'0'*64), '\n'.join(line for line in log.splitlines() if 'stage=answer_received' not in line)]:
            with self.assertRaises(ValueError):
                populations.native(sources,altered,'boot')

    def test_clock_phase_mapping_is_an_interval_instead_of_wall_subtraction(self):
        pair={'edge':{'host_request_before_ns':100,'host_request_after_ns':120,
                      'monotonic_before_ns':10,'monotonic_after_ns':12},
              'server':{'host_request_before_ns':130,'host_request_after_ns':150,
                        'monotonic_before_ns':1000,'monotonic_after_ns':1002}}
        low,high=populations.window({'monotonic_ns':20},[pair],'server')
        self.assertEqual(low,20+88-(-850)+15*10**9)
        self.assertEqual(high,20+110-(-872)+135*10**9)
