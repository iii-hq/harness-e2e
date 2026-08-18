import asyncio
import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).parents[1]))
from src.cancellation import Operation


async def check():
    operation = Operation()
    started = asyncio.Event()
    task = asyncio.create_task(operation.run(started))
    await started.wait()
    task.cancel()
    try:
        await task
    except asyncio.CancelledError:
        pass
    return operation.resource_open is False and operation.state == "cancelled"


if not asyncio.run(check()):
    print("cancellation_cleanup:FAIL")
    raise SystemExit(1)
print("cancellation_cleanup:PASS")
