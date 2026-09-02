import unittest

from src.event_store import EventStore
from src.replay import replay_all


class BatchReplayTests(unittest.TestCase):
    def test_replays_every_event_in_order_across_pages(self):
        observed = []
        count = replay_all(EventStore(list(range(7))), observed.append, batch_size=3)
        self.assertEqual(observed, list(range(7)))
        self.assertEqual(count, 7)

    def test_can_resume_after_a_cursor(self):
        observed = []
        count = replay_all(EventStore(list(range(6))), observed.append, batch_size=2, start_cursor=2)
        self.assertEqual(observed, [3, 4, 5])
        self.assertEqual(count, 3)


if __name__ == "__main__":
    unittest.main()
