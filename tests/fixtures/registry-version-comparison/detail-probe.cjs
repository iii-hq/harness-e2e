/* Shared browser-side helpers for locating and validating one expanded timeout change. */
globalThis.registryDetailProbe = (() => {
  function text(element) {
    return element?.innerText || element?.textContent || '';
  }

  function visible(element) {
    return element?.checkVisibility?.() !== false;
  }

  function disclosure(element, document) {
    if (element.tagName === 'SUMMARY' && element.parentElement?.tagName === 'DETAILS') {
      return element.parentElement;
    }
    const controlled = element.getAttribute?.('aria-controls');
    return controlled ? document.getElementById(controlled) : null;
  }

  function boundedTimeoutRow(element, document) {
    const controlled = disclosure(element, document);
    if (/timeout/i.test(text(element)) && controlled) return controlled;
    if (controlled && /timeout/i.test(text(controlled))) return controlled;

    const immediateItem = controlled?.parentElement || element.parentElement;
    if (immediateItem && immediateItem !== document.body && /timeout/i.test(text(immediateItem))) {
      return immediateItem;
    }

    const semanticItem = element.closest?.('article, li, tr, [role="row"], [data-change-row]');
    return semanticItem && /timeout/i.test(text(semanticItem)) ? semanticItem : null;
  }

  function candidates(document) {
    return [...document.querySelectorAll('summary, button[aria-expanded], button[aria-controls]')]
      .filter(element => visible(element) && boundedTimeoutRow(element, document));
  }

  function state(element, document) {
    const content = disclosure(element, document);
    let expanded = false;
    if (element.tagName === 'SUMMARY') expanded = content?.open === true;
    else if (element.getAttribute?.('aria-expanded') === 'true') expanded = visible(content);
    const localText = expanded ? text(content) : '';
    return {
      expanded,
      localText,
      beforePresent: /3[,.]?000/.test(localText),
      afterPresent: /5[,.]?000/.test(localText),
    };
  }

  return { candidates, state };
})();
