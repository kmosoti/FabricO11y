"""Fixed offered HTTP demand for a private native plan comparison."""
from pathlib import Path
import queue
import sys
import threading
import time

sys.path.insert(0, str(Path(__file__).resolve().parent.parent/'dev_small'))
import measurement
from measurement import observation


class Observer(measurement.Observer):
    def start(self, api, epoch, nodes):
        self.api, self.epoch = api, epoch
        origin = time.monotonic_ns()

        def client():
            for tick in range(200):
                if self.done.wait(max(0, (origin + tick*10**9-time.monotonic_ns())/1e9)):
                    break
                scheduled = epoch + tick*10**9
                if tick % 3 == 0:
                    shape, query = 'recent_logs', dict(observation.QUERY,
                        from_ns=scheduled-10*10**9, to_ns=scheduled)
                elif tick % 3 == 1:
                    shape, query = 'absent_text', dict(observation.QUERY,
                        contains='ABSENT-profile-sentinel')
                else:
                    shape, query = 'cpu_metrics', dict(observation.QUERY,
                        kind='metrics', name=observation.METRIC)
                self.request(shape, query, scheduled)

        def visibility():
            for _ in range(36):
                while not self.done.is_set():
                    try:
                        target = self.targets.get(timeout=.1)
                        break
                    except queue.Empty:
                        continue
                else:
                    return
                scheduled = target['source_ns'] + 3*10**9
                if self.done.wait(max(0, (scheduled-time.time_ns())/1e9)):
                    return
                row = self.request('visibility', dict(observation.QUERY,
                    node='node'+target['tag'][:2], contains='load-'+target['tag']+' ',
                    limit=2), scheduled)
                if row['answer'] and observation.exact_live(row['answer'], target['sha256']):
                    target['visible_ns'] = row['end_ns']
                    target['row'] = row['answer']['rows'][0]

        for function in (client, visibility):
            thread = threading.Thread(target=function)
            thread.start()
            self.threads.append(thread)


def controls():
    # The independently implemented auditor owns count/shape/clock controls.
    import importlib.util
    spec = importlib.util.spec_from_file_location('plan_audit', Path(__file__).with_name('audit.py'))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.controls()
