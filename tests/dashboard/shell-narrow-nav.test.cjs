// Audits S-01 / S-02 / RD-01 on layout A: the sections and the section's
// actions live in the Console's PageHeader. Which form they take (tabs, a
// section menu below 720px of pane, a sheet on a phone) is decided by the
// shell from the pane width it measures, never by a Tailwind utility, and
// the actions fold into a menu so the header's close control stays in view.
// DashboardShell.test.tsx renders the same states; these checks keep the
// source from sliding back to the patterns the audits removed.
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");

const componentsDir = path.join(__dirname, "..", "..", "dashboard", "src", "components");
const read = (...parts) => fs.readFileSync(path.join(componentsDir, ...parts), "utf8");
const shellCss = read("dashboard-shell.css");
const shellTsx = read("DashboardShell.tsx");
const sectionNav = read("shell", "SectionNav.tsx");
const headerActions = read("shell", "HeaderActions.tsx");

/** Every rule body whose selector list is exactly `selector`. */
function rules(selector) {
  const escaped = selector.replace(/[.[\]"=:()]/g, "\\$&");
  return [...shellCss.matchAll(new RegExp(`\\n\\s*${escaped}\\s*\\{([^}]*)\\}`, "g"))].map((match) => match[1]);
}

test("the shell decides the narrow and phone forms from the pane width (S-01 / RD-01)", () => {
  assert.match(shellTsx, /useContainerNarrow\(720\)/, "the section form follows the pane, not the viewport");
  assert.match(shellTsx, /data-narrow=\{narrow \? 'true' : 'false'\}/);
  assert.match(shellTsx, /<SectionNav[^>]*\bnarrow=\{narrow\}[^>]*\bphone=\{phone\}/);
  assert.match(shellTsx, /<HeaderActions[^>]*\bnarrow=\{narrow\}[^>]*\bphone=\{phone\}/);
  // Phone: a sheet; narrow: a menu naming the current section; wide: links.
  assert.match(sectionNav, /if \(phone\) \{[\s\S]*?<BottomSheet\b/);
  assert.match(sectionNav, /if \(narrow\) \{[\s\S]*?<DropdownMenu\b[\s\S]*?className="harness-e2e-section-menu"/);
  assert.match(sectionNav, /className="harness-e2e-section-tabs"/);
  assert.match(sectionNav, /aria-current=\{section\.value === current \? 'page' : undefined\}/);
});

test("no Tailwind `hidden` utility shows or hides the navigation or the actions", () => {
  for (const [file, source] of [
    ["DashboardShell.tsx", shellTsx],
    ["SectionNav.tsx", sectionNav],
    ["HeaderActions.tsx", headerActions],
  ]) {
    assert.doesNotMatch(source, /className=[{"'`][^>]*\bhidden\b/, `${file} hides chrome with the \`hidden\` utility`);
  }
  for (const selector of [".harness-e2e-sections", ".harness-e2e-section-tabs", ".harness-e2e-header-actions"]) {
    for (const body of rules(selector)) assert.doesNotMatch(body, /display:\s*none/, selector);
  }
});

// S-02: the actions sit in the host PageHeader's actions slot, beside the
// close control; below 720px only the primary stays next to ⋯, and on a
// phone ⋯ is alone while the primary pins to the bottom of the pane.
test("page actions go through HeaderActions into the PageHeader actions slot and fold when narrow (S-02)", () => {
  assert.match(shellTsx, /<PageHeader[\s\S]*?actions=\{\s*<HeaderActions\b[\s\S]*?onClose=\{onRequestClose\}[\s\S]*?<\/PageHeader>/);
  assert.match(shellTsx, /<PinnedPrimary\b/);
  assert.doesNotMatch(shellTsx, /PageActionsBar|harness-e2e-page-actions/);
  assert.match(headerActions, /const folded = narrow \|\| phone/);
  assert.match(headerActions, /const inHeader = folded\s*\?\s*\(phone \? \[\] : primary \? \[primary\] : \[\]\)\s*:\s*actions/);
  assert.match(headerActions, /<DropdownMenuTrigger asChild>[\s\S]*?aria-label="More actions"/);
  assert.match(headerActions, /if \(!phone \|\| !primary\) return null/);
  assert.match(rules(".harness-e2e-pinned-primary").join(""), /position:\s*sticky[^}]*bottom:\s*0/);
});

// Redesign canvas: sentence-case Inter sections with an ink underline on the
// current one; the actions are Inter too.
test("section links and header actions are sentence-case sans, not mono lowercase", () => {
  for (const selector of [".harness-e2e-section-tab", ".harness-e2e-section-menu", ".harness-e2e-header-action"]) {
    const bodies = rules(selector);
    assert.ok(bodies.length > 0, `${selector} has no rule`);
    assert.ok(
      bodies.some((body) => /font-family:\s*var\(--font-sans\)/.test(body)),
      `${selector} is not set in var(--font-sans)`,
    );
    for (const body of bodies) assert.doesNotMatch(body, /text-transform:\s*lowercase/, selector);
  }
  assert.match(
    rules('.harness-e2e-section-tab[aria-current="page"]::after').join(""),
    /height:\s*2px[^}]*background:\s*var\(--color-ink\)/,
  );
  for (const label of ["Tests", "Executions", "Trends", "Suites", "Stacks"]) {
    assert.match(shellTsx, new RegExp(`label: '${label}'`));
  }
  assert.doesNotMatch(sectionNav, /font-mono|lowercase/);
  assert.doesNotMatch(headerActions, /font-mono|lowercase/);
});
