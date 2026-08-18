import pathlib
import sys
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).parents[1]))
from src.serialization import decode_event, encode_event


class SerializationPublicTest(unittest.TestCase):
    def test_new_optional_field_round_trips(self):
        event = {"id": "e1", "name": "created", "trace_id": "t1"}
        self.assertEqual(decode_event(encode_event(event)), event)

    def test_legacy_event_still_round_trips(self):
        event = {"id": "e1", "name": "created"}
        self.assertEqual(decode_event(encode_event(event)), event)


if __name__ == "__main__":
    unittest.main()
