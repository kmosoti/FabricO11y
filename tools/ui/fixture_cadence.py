"""Pace explicit benign fixture actions; never retry or rewrite a response."""
import time


# Native LIMITS order: rate, burst, peer_live, global_live; the CI drift check binds it.
PEER_ADMISSION = {
    'public': {'limits': (2, 4, 2, 8)},
    'private': {'limits': (8, 8, 4, 16)},
}
PEER_ADMISSION['public']['action_interval_s'] = 1 / PEER_ADMISSION['public']['limits'][0] + .05
PEER_ADMISSION['private'].update(command_interval_s=4 / PEER_ADMISSION['private']['limits'][0],
                                 page_interval_s=2 / PEER_ADMISSION['private']['limits'][0])


class FixtureCadence:
    def __init__(self, clock=time.monotonic, sleep=time.sleep, *,
                 interval=PEER_ADMISSION['public']['action_interval_s']):
        self.clock, self.sleep = clock, sleep
        self.interval = interval  # In the injected clock/sleep units (seconds or nanoseconds).
        self.last = None

    def pace(self):
        if self.last is not None:
            remaining = self.interval - (self.clock() - self.last)
            if remaining > 0:
                self.sleep(remaining)
        self.last = self.clock()
