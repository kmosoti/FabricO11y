"""Fixed-query caller pacing; inherited pagination includes every admission wait."""
import time
import urllib.error

from release_runtime import WorkloadReader


class PacedQueryReader(WorkloadReader):
    def __init__(self, reader):
        super().__init__(reader.origin, reader.context, reader.token)
        self.observations = reader.observations
        self.page_schedule = []
        self._previous_page_attempt = None

    def query(self, body):
        if len(self.page_schedule) >= 10000:
            raise RuntimeError('fixed-query page schedule exceeds 10000 requests')
        requested = time.monotonic_ns()
        previous = self._previous_page_attempt
        if previous is not None:
            time.sleep(max(0, (previous + 250_000_000 - requested) / 1_000_000_000))
        attempted = time.monotonic_ns()
        self._previous_page_attempt = attempted
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
