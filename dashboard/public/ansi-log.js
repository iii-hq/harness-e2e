(function initHarnessAnsiLog(root, factory) {
  const api = factory();
  if (typeof module === "object" && module.exports) {
    module.exports = api;
  }
  root.HarnessAnsiLog = api;
})(typeof globalThis !== "undefined" ? globalThis : this, function ansiLogFactory() {
  "use strict";

  const ANSI_SGR = /\u001b\[([0-9;]*)m/g;
  const COLOR_CLASSES = Object.freeze({
    30: "ansi-fg-black",
    31: "ansi-fg-red",
    32: "ansi-fg-green",
    33: "ansi-fg-yellow",
    34: "ansi-fg-blue",
    35: "ansi-fg-magenta",
    36: "ansi-fg-cyan",
    37: "ansi-fg-white",
    90: "ansi-fg-bright-black",
    91: "ansi-fg-bright-red",
    92: "ansi-fg-bright-green",
    93: "ansi-fg-bright-yellow",
    94: "ansi-fg-bright-blue",
    95: "ansi-fg-bright-magenta",
    96: "ansi-fg-bright-cyan",
    97: "ansi-fg-bright-white",
  });

  function reset(style) {
    style.bold = false;
    style.dim = false;
    style.italic = false;
    style.color = "";
  }

  function applyCodes(style, rawCodes) {
    const codes = rawCodes === "" ? [0] : rawCodes.split(";").map(Number);
    codes.forEach((code) => {
      if (code === 0) reset(style);
      else if (code === 1) style.bold = true;
      else if (code === 2) style.dim = true;
      else if (code === 3) style.italic = true;
      else if (code === 22) {
        style.bold = false;
        style.dim = false;
      } else if (code === 23) style.italic = false;
      else if (code === 39) style.color = "";
      else if (COLOR_CLASSES[code]) style.color = COLOR_CLASSES[code];
    });
  }

  function className(style) {
    return [
      style.color,
      style.bold && "ansi-bold",
      style.dim && "ansi-dim",
      style.italic && "ansi-italic",
    ]
      .filter(Boolean)
      .join(" ");
  }

  function tokenizeAnsiLog(value) {
    const input = typeof value === "string" ? value : String(value ?? "");
    const style = {};
    reset(style);
    const tokens = [];
    let offset = 0;

    function append(text) {
      if (!text) return;
      const classes = className(style);
      const previous = tokens[tokens.length - 1];
      if (previous?.className === classes) {
        previous.text += text;
      } else {
        tokens.push({ text, className: classes });
      }
    }

    for (const match of input.matchAll(ANSI_SGR)) {
      append(input.slice(offset, match.index));
      applyCodes(style, match[1]);
      offset = match.index + match[0].length;
    }
    append(input.slice(offset));
    return tokens;
  }

  return { tokenizeAnsiLog };
});
