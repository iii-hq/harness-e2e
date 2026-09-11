/* Trusted browser::execute body. The controller prepends `const capture = ...`. */
return await (async () => {
  const versions = ['0.9.0', '1.0.0', '1.1.0', '2.0.0'];
  const deadline = Date.now() + 20_000;

  function visibleText() {
    return document.body?.innerText || '';
  }

  function selectedVersions() {
    return [...document.querySelectorAll('select')].map(select => select.value);
  }

  let detailClicked = false;
  let detailExpanded = false;
  let detailValues = [];
  let reason = 'Timed out waiting for the expected UI state';
  while (Date.now() < deadline) {
    const text = visibleText();
    const selected = selectedVersions();
    const hasChangelog = /changelog/i.test(text);

    if (capture.kind === 'history') {
      if (hasChangelog && versions.every(version => text.includes(version))) reason = null;
    } else if (capture.kind === 'comparison') {
      const pairSelected = selected.includes(capture.from) && selected.includes(capture.to);
      const expectedContent = capture.expected.some(value => text.includes(value));
      if (hasChangelog && pairSelected && expectedContent) reason = null;
    } else if (capture.kind === 'detail') {
      for (const control of registryDetailProbe.candidates(document)) {
        control.click();
        detailClicked = true;
        await sleep(250);
        const detail = registryDetailProbe.state(control, document);
        detailExpanded = detail.expanded;
        detailValues = [detail.beforePresent, detail.afterPresent];
        if (detailExpanded && detailValues.every(Boolean)) {
          reason = null;
          break;
        }
      }
    } else if (capture.kind === 'missing') {
      const failed = /not[ _-]?found|missing|unavailable|unable|error|failed/i.test(text);
      if (hasChangelog && failed) reason = null;
    }

    if (!reason) break;
    await sleep(200);
  }

  const text = visibleText();
  return {
    status: reason ? 'failed' : 'passed',
    reason,
    state: {
      url: location.href,
      title: document.title,
      selected_versions: selectedVersions(),
      detail_clicked: detailClicked,
      detail_expanded: detailExpanded,
      detail_values_present: detailValues,
      body_excerpt: text.slice(0, 4000),
    },
  };
})();
