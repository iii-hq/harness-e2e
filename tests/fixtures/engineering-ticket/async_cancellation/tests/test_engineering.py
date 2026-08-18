import asyncio
import pathlib
import sys
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).parents[1]))
from src.cancellation import Operation


class CancellationPublicTest(unittest.IsolatedAsyncioTestCase):
    async def test_cancel_closes_resource_and_sets_terminal_state(self):
        operation = Operation()
        started = asyncio.Event()
        task = asyncio.create_task(operation.run(started))
        await started.wait()
        task.cancel()
        with self.assertRaises(asyncio.CancelledError):
            await task
        self.assertFalse(operation.resource_open)
        self.assertEqual(operation.state, "cancelled")


if __name__ == "__main__":
    unittest.main()
