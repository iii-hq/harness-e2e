const fs = require("node:fs");
const path = require("node:path");

function loadBrowserModule(relativePath) {
  const filename = path.join(__dirname, "..", "..", relativePath);
  const source = fs.readFileSync(filename, "utf8");
  const moduleRecord = { exports: {} };
  const evaluate = new Function("module", "exports", source);
  evaluate(moduleRecord, moduleRecord.exports);
  return moduleRecord.exports;
}

module.exports = { loadBrowserModule };
