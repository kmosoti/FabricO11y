import base64
import hashlib
from pathlib import Path
import sys
import unittest
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
import populations
import producer


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
