const test = require("node:test");
const assert = require("node:assert/strict");
const { loadBrowserModule } = require("./load-browser-module.cjs");

const { tokenizeAnsiLog } = loadBrowserModule(
  "dashboard/public/ansi-log.js",
);

test("converts tracing ANSI styles into safe presentation tokens", () => {
  const raw =
    "\u001b[2m2026-08-14T16:30:05Z\u001b[0m " +
    "\u001b[32mINFO\u001b[0m " +
    "\u001b[2mtelemetry\u001b[0m: " +
    "\u001b[3mname\u001b[0m=ready";

  const tokens = tokenizeAnsiLog(raw);

  assert.equal(
    tokens.map((token) => token.text).join(""),
    "2026-08-14T16:30:05Z INFO telemetry: name=ready",
  );
  assert.deepEqual(
    tokens.filter((token) => token.className),
    [
      { text: "2026-08-14T16:30:05Z", className: "ansi-dim" },
      { text: "INFO", className: "ansi-fg-green" },
      { text: "telemetry", className: "ansi-dim" },
      { text: "name", className: "ansi-italic" },
    ],
  );
  assert.equal(tokens.some((token) => token.text.includes("\u001b")), false);
});

test("supports combined and bright ANSI styles", () => {
  assert.deepEqual(tokenizeAnsiLog("\u001b[1;91mFAIL\u001b[22;39m plain"), [
    { text: "FAIL", className: "ansi-fg-bright-red ansi-bold" },
    { text: " plain", className: "" },
  ]);
});
