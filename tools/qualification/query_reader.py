"""Fixed-query caller pacing; inherited pagination includes every admission wait."""
import time
import sys
import urllib.error

from release_runtime import WorkloadReader, ROOT
sys.path.insert(0, str(ROOT / 'tools/ui'))
from fixture_cadence import FixtureCadence, PEER_ADMISSION


class PacedQueryReader(WorkloadReader):
    def __init__(self, reader):
        super().__init__(reader.origin, reader.context, reader.token)
        self.observations = reader.observations
        self.page_schedule = []
        self._page_cadence = FixtureCadence(
            lambda: time.monotonic_ns(), lambda delay: time.sleep(delay / 1_000_000_000),
            interval=round(PEER_ADMISSION['private']['page_interval_s'] * 1_000_000_000))

    def query(self, body):
        if len(self.page_schedule) >= 10000:
            raise RuntimeError('fixed-query page schedule exceeds 10000 requests')
        requested = time.monotonic_ns()
        self._page_cadence.pace()
        attempted = self._page_cadence.last
        event = {'requested_monotonic_ns': requested,
                 'attempted_monotonic_ns': attempted,
                 'admission_wait_ns': attempted - requested}
        self.page_schedule.append(event)
        try:
            answer = super().query(body)
        except urllib.error.HTTPError as error:
            event.update(outcome='http_error', status=error.code)
            raise
        except BaseException as error:
            event.update(outcome='failed', failure_type=type(error).__name__)
            raise
        else:
            event['outcome'] = 'response_decoded'
            return answer
        finally:
            event['completed_monotonic_ns'] = time.monotonic_ns()
