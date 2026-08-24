# Production operations handbook v2

Status: current
Authority: operations

Operators watch the canary for fifteen minutes before increasing traffic. The observation window
is measured from the first healthy canary instance. During the window, the release dashboard must
show zero critical alerts, a stable request error rate, and error-budget burn below two percent.

If any critical alert fires, traffic returns to the previous immutable build and the current
release attempt is closed. The operations log records the build identity, window timestamps, and
rollback receipt.
