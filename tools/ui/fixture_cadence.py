"""Pace explicit benign fixture actions; never retry or rewrite a response."""
import time


class FixtureCadence:
    def __init__(self, clock=time.monotonic, sleep=time.sleep):
        self.clock, self.sleep = clock, sleep
        self.last = None

    def pace(self):
        if self.last is not None:
            remaining = .550 - (self.clock() - self.last)
            if remaining > 0:
                self.sleep(remaining)
        self.last = self.clock()
