(function defineHarnessLocalRunner(global) {
  "use strict";

  const elements = {
    advanced: document.querySelector("#local-advanced"),
    cancel: document.querySelector("#local-run-cancel"),
    catalogIndicator: document.querySelector("#local-catalog-indicator"),
    catalogRefresh: document.querySelector("#local-catalog-refresh"),
    catalogStatus: document.querySelector("#local-catalog-status"),
    connectionUrl: document.querySelector("#local-connection-url"),
    form: document.querySelector("#local-run-form"),
    judge: document.querySelector("#local-judge"),
    runError: document.querySelector("#local-run-error"),
    runLog: document.querySelector("#local-run-log"),
    runLogShell: document.querySelector("#local-run-log-shell"),
    runStatus: document.querySelector("#local-run-status"),
    scenarioAll: document.querySelector("#local-scenario-all"),
    scenarioNone: document.querySelector("#local-scenario-none"),
    scenarioOptions: document.querySelector("#local-scenario-options"),
    scenarioPicker: document.querySelector("#local-scenario-picker"),
    scenarioSummary: document.querySelector("#local-scenario-summary"),
    subject: document.querySelector("#local-subject"),
    subjectOptions: document.querySelector("#local-subject-options"),
    subjectPicker: document.querySelector("#local-subject-picker"),
    subjectSearch: document.querySelector("#local-subject-search"),
    subjectSummary: document.querySelector("#local-subject-summary"),
    judgeOptions: document.querySelector("#local-judge-options"),
    judgePicker: document.querySelector("#local-judge-picker"),
    judgeSearch: document.querySelector("#local-judge-search"),
    judgeSummary: document.querySelector("#local-judge-summary"),
    submit: document.querySelector("#local-run-submit"),
  };

  const modelPickers = [
    {
      includeAutomatic: false,
      options: elements.subjectOptions,
      picker: elements.subjectPicker,
      search: elements.subjectSearch,
      select: elements.subject,
      summary: elements.subjectSummary,
    },
    {
      includeAutomatic: true,
      options: elements.judgeOptions,
      picker: elements.judgePicker,
      search: elements.judgeSearch,
      select: elements.judge,
      summary: elements.judgeSummary,
    },
  ];

  let pollTimer = null;
  let catalogReady = false;
  let catalogLoading = false;
  let jobActive = false;
  let defaults = {};
  let initialized = false;

  function renderRunLog(value) {
    const tokens = global.HarnessAnsiLog?.tokenizeAnsiLog(value) || [
      { text: value || "", className: "" },
    ];
    const fragment = document.createDocumentFragment();
    tokens.forEach((token) => {
      if (!token.className) {
        fragment.append(document.createTextNode(token.text));
        return;
      }
      const span = document.createElement("span");
      span.className = token.className;
      span.textContent = token.text;
      fragment.append(span);
    });
    elements.runLog.replaceChildren(fragment);
  }

  function formField(name) {
    return elements.form.elements.namedItem(name);
  }

  function applyDefaults(nextDefaults) {
    defaults = { ...defaults, ...(nextDefaults || {}) };
    Object.entries(nextDefaults || {}).forEach(([name, value]) => {
      const field = formField(name);
      if (field && !field.value && value !== null && value !== undefined) {
        field.value = String(value);
      }
    });
    const url = formField("url")?.value || nextDefaults?.url || "";
    if (url) elements.connectionUrl.textContent = url;
  }

  function setControls(active) {
    jobActive = active;
    for (const field of elements.form.elements) {
      if (field !== elements.cancel) field.disabled = active;
    }
    elements.subject.disabled = active || !catalogReady;
    elements.judge.disabled = active || !catalogReady;
    modelPickers.forEach((picker) => {
      const disabled = active || !catalogReady;
      picker.picker.classList.toggle("local-picker-disabled", disabled);
      picker.picker.setAttribute("aria-disabled", String(disabled));
      picker.picker.querySelector(":scope > summary").tabIndex = disabled ? -1 : 0;
      picker.search.disabled = disabled;
      picker.options.querySelectorAll("button").forEach((button) => {
        button.disabled = disabled;
      });
      if (disabled) picker.picker.open = false;
    });
    elements.submit.disabled = active || !catalogReady;
    elements.catalogRefresh.disabled = active || catalogLoading;
    elements.scenarioPicker.classList.toggle(
      "local-picker-disabled",
      active || !catalogReady,
    );
    elements.scenarioPicker.setAttribute(
      "aria-disabled",
      String(active || !catalogReady),
    );
  }

  function renderJob(response) {
    applyDefaults(response?.defaults);
    const job = response?.job;
    const active = ["running", "cancelling"].includes(job?.status);
    setControls(active);
    elements.cancel.hidden = !active;
    elements.runError.hidden = !job?.error;
    elements.runError.textContent = job?.error || "";
    elements.runLogShell.hidden = !job?.log;
    renderRunLog(job?.log || "");
    if (job?.log && active) elements.runLogShell.open = true;
    elements.runStatus.textContent = !job
      ? "Ready"
      : {
          running: "Running…",
          cancelling: "Cancelling…",
          cancelled: "Cancelled",
          completed: "Results saved",
          failed: "Runner failed",
        }[job.status] || job.status;
    if (active) {
      clearTimeout(pollTimer);
      pollTimer = setTimeout(refreshJob, 1_000);
    } else if (job?.status === "completed" && job.id) {
      const reloadKey = "harness-e2e-local-last-reload";
      if (sessionStorage.getItem(reloadKey) !== job.id) {
        sessionStorage.setItem(reloadKey, job.id);
        global.location.reload();
      }
    }
  }

  async function api(path, options = {}) {
    const response = await fetch(path, {
      cache: "no-store",
      headers: { "Content-Type": "application/json" },
      ...options,
    });
    const payload = await response.json().catch(() => ({}));
    if (!response.ok) {
      throw new Error(payload.error || `Request failed (${response.status})`);
    }
    return payload;
  }

  async function refreshJob() {
    try {
      const response = await api("./api/local/run");
      renderJob(response);
      return response;
    } catch (error) {
      elements.runError.hidden = false;
      elements.runError.textContent = error.message;
      elements.runStatus.textContent = "Unavailable";
      return null;
    }
  }

  function modelKey(model) {
    return `${model.provider}\n${model.model}`;
  }

  function selectedModel(select) {
    const option = select.selectedOptions[0];
    return option?.dataset.model && option?.dataset.provider
      ? { model: option.dataset.model, provider: option.dataset.provider }
      : null;
  }

  function pickerForSelect(select) {
    return modelPickers.find((picker) => picker.select === select);
  }

  function normalizedSearch(value) {
    return String(value || "")
      .normalize("NFKD")
      .replace(/[\u0300-\u036f]/g, "")
      .toLocaleLowerCase();
  }

  function updateModelPickerSelection(picker) {
    const option = picker.select.selectedOptions[0];
    const model = selectedModel(picker.select);
    picker.summary.textContent = model
      ? `${model.provider} / ${model.model}`
      : "Automatic · use subject model";
    picker.summary.title = model ? `${model.provider} / ${model.model}` : "";
    picker.options
      .querySelectorAll("[data-option-value]")
      .forEach((button) => {
        const selected = button.dataset.optionValue === (option?.value || "");
        button.setAttribute("aria-selected", String(selected));
        button.classList.toggle("selected", selected);
      });
  }

  function filterModelPicker(picker) {
    const query = normalizedSearch(picker.search.value.trim());
    let visible = 0;
    picker.options
      .querySelectorAll(":scope > .local-model-option")
      .forEach((button) => {
        const matches = !query || normalizedSearch(button.dataset.search).includes(query);
        button.hidden = !matches;
        if (matches) visible += 1;
      });
    picker.options
      .querySelectorAll(".local-model-provider")
      .forEach((providerGroup) => {
        let providerVisible = 0;
        providerGroup
          .querySelectorAll(".local-model-option")
          .forEach((button) => {
            const matches = !query || normalizedSearch(button.dataset.search).includes(query);
            button.hidden = !matches;
            if (matches) providerVisible += 1;
          });
        providerGroup.hidden = providerVisible === 0;
        visible += providerVisible;
        providerGroup.open = query
          ? providerVisible > 0
          : Boolean(providerGroup.querySelector('[aria-selected="true"]'));
      });
    const empty = picker.options.querySelector(".local-model-empty");
    empty.hidden = visible > 0;
    empty.textContent = query
      ? `No models match “${picker.search.value.trim()}”.`
      : "No registered models.";
  }

  function makeModelButton(option, label, searchText, className = "") {
    const button = document.createElement("button");
    button.type = "button";
    button.className = `local-model-option ${className}`.trim();
    button.dataset.optionValue = option.value;
    button.dataset.search = searchText;
    button.setAttribute("role", "option");
    button.setAttribute("aria-selected", "false");
    const text = document.createElement("span");
    text.textContent = label;
    const check = document.createElement("span");
    check.className = "local-model-check";
    check.textContent = "✓";
    check.setAttribute("aria-hidden", "true");
    button.append(text, check);
    return button;
  }

  function renderModelPicker(select, models, { includeAutomatic = false } = {}) {
    const picker = pickerForSelect(select);
    picker.options.replaceChildren();
    if (includeAutomatic) {
      const automatic = makeModelButton(
        select.options[0],
        "Automatic · use subject model",
        "automatic default subject model",
        "local-model-automatic",
      );
      picker.options.append(automatic);
    }
    const providers = new Map();
    models.forEach((model) => {
      if (!providers.has(model.provider)) providers.set(model.provider, []);
      providers.get(model.provider).push(model);
    });
    providers.forEach((providerModels, provider) => {
      const group = document.createElement("details");
      group.className = "local-model-provider";
      group.dataset.provider = provider;
      const summary = document.createElement("summary");
      const name = document.createElement("strong");
      name.textContent = provider;
      const count = document.createElement("span");
      count.textContent = `${providerModels.length} model${providerModels.length === 1 ? "" : "s"}`;
      summary.append(name, count);
      const choices = document.createElement("div");
      choices.className = "local-model-provider-options";
      choices.setAttribute("role", "group");
      choices.setAttribute("aria-label", provider);
      providerModels.forEach((model) => {
        const option = [...select.options].find(
          (candidate) =>
            candidate.dataset.model === model.model &&
            candidate.dataset.provider === model.provider,
        );
        const button = makeModelButton(
          option,
          model.model,
          `${model.provider} ${model.model}`,
        );
        button.title = `${model.provider} / ${model.model}`;
        choices.append(button);
      });
      group.append(summary, choices);
      picker.options.append(group);
    });
    const empty = document.createElement("p");
    empty.className = "local-model-empty";
    empty.hidden = true;
    picker.options.append(empty);
    updateModelPickerSelection(picker);
    filterModelPicker(picker);
  }

  function fillModelSelect(select, models, { includeAutomatic = false } = {}) {
    const selected = selectedModel(select);
    const preferredKey =
      (selected && modelKey(selected)) ||
      localStorage.getItem("harness-e2e-local-subject") ||
      (defaults.model && defaults.provider
        ? modelKey({ model: defaults.model, provider: defaults.provider })
        : "");
    select.replaceChildren();
    if (includeAutomatic) {
      const automatic = document.createElement("option");
      automatic.value = "";
      automatic.textContent = "Use subject model when required";
      select.append(automatic);
    }
    models.forEach((model, index) => {
      const option = document.createElement("option");
      option.value = `model-${index}`;
      option.dataset.model = model.model;
      option.dataset.provider = model.provider;
      option.textContent = `${model.provider} / ${model.model}`;
      option.selected = !includeAutomatic && modelKey(model) === preferredKey;
      select.append(option);
    });
    if (!includeAutomatic && select.selectedIndex < 0 && select.options.length) {
      select.selectedIndex = 0;
    }
    renderModelPicker(select, models, { includeAutomatic });
  }

  function scenarioInputs() {
    return [...elements.scenarioOptions.querySelectorAll("input[type=checkbox]")];
  }

  function updateScenarioSummary() {
    const inputs = scenarioInputs();
    const selected = inputs.filter((input) => input.checked).length;
    if (!inputs.length) {
      elements.scenarioSummary.textContent = catalogLoading
        ? "Loading scenarios…"
        : "Catalog unavailable";
      elements.submit.disabled = true;
      return;
    }
    elements.scenarioSummary.textContent =
      selected === inputs.length
        ? `All ${inputs.length} scenarios`
        : `${selected} of ${inputs.length} scenarios`;
    elements.submit.disabled = jobActive || !catalogReady || selected === 0;
  }

  function fillScenarios(scenarios) {
    const previous = new Set(
      scenarioInputs().filter((input) => input.checked).map((input) => input.value),
    );
    elements.scenarioOptions.replaceChildren();
    scenarios.forEach((scenarioId, index) => {
      const label = document.createElement("label");
      label.className = "local-scenario-option";
      const input = document.createElement("input");
      input.type = "checkbox";
      input.name = "scenario";
      input.value = scenarioId;
      input.id = `local-scenario-${index}`;
      input.checked = previous.has(scenarioId);
      const text = document.createElement("span");
      text.textContent = scenarioId.replaceAll("_", " ");
      text.title = scenarioId;
      label.append(input, text);
      elements.scenarioOptions.append(label);
    });
    updateScenarioSummary();
  }

  async function refreshCatalog() {
    const url = formField("url")?.value || defaults.url || "";
    elements.connectionUrl.textContent = url;
    elements.catalogStatus.textContent = "Discovering the running Harness…";
    elements.catalogIndicator.className = "local-connection-dot";
    catalogLoading = true;
    catalogReady = false;
    setControls(jobActive);
    try {
      const query = new URLSearchParams({ url });
      const catalog = await api(`./api/local/catalog?${query}`);
      fillModelSelect(elements.subject, catalog.models);
      fillModelSelect(elements.judge, catalog.models, { includeAutomatic: true });
      fillScenarios(catalog.scenarios);
      catalogReady = true;
      elements.catalogIndicator.className = "local-connection-dot connected";
      elements.catalogStatus.textContent =
        `${catalog.models.length} registered model${catalog.models.length === 1 ? "" : "s"} · ${catalog.scenarios.length} scenarios`;
      elements.runError.hidden = true;
    } catch (error) {
      elements.catalogIndicator.className = "local-connection-dot failed";
      elements.catalogStatus.textContent = "Could not read the Harness catalog";
      elements.runError.hidden = false;
      elements.runError.textContent = error.message;
      elements.scenarioSummary.textContent = "Catalog unavailable";
      elements.advanced.open = true;
    } finally {
      catalogLoading = false;
      setControls(jobActive);
      updateScenarioSummary();
    }
  }

  function initialize() {
    if (initialized) return;
    initialized = true;
    modelPickers.forEach((picker) => {
      picker.options.addEventListener("click", (event) => {
        const button = event.target.closest("[data-option-value]");
        if (!button || button.disabled) return;
        picker.select.value = button.dataset.optionValue;
        picker.select.dispatchEvent(new Event("change", { bubbles: true }));
        updateModelPickerSelection(picker);
        picker.search.value = "";
        filterModelPicker(picker);
        picker.picker.open = false;
        picker.picker.querySelector(":scope > summary").focus();
      });
      picker.search.addEventListener("input", () => filterModelPicker(picker));
      picker.search.addEventListener("keydown", (event) => {
        if (event.key === "Enter") event.preventDefault();
        if (event.key !== "Escape") return;
        if (picker.search.value) {
          picker.search.value = "";
          filterModelPicker(picker);
        } else {
          picker.picker.open = false;
          picker.picker.querySelector(":scope > summary").focus();
        }
      });
      picker.picker.addEventListener("toggle", () => {
        if (!picker.picker.open) return;
        modelPickers.forEach((other) => {
          if (other !== picker) other.picker.open = false;
        });
        requestAnimationFrame(() => picker.search.focus());
      });
    });
    elements.form.addEventListener("submit", async (event) => {
      event.preventDefault();
      const values = new FormData(elements.form);
      const subject = selectedModel(elements.subject);
      const judge = selectedModel(elements.judge);
      const scenarios = scenarioInputs()
        .filter((input) => input.checked)
        .map((input) => input.value);
      try {
        if (!subject) throw new Error("Select a registered subject model.");
        if (!scenarios.length) throw new Error("Select at least one scenario.");
        localStorage.setItem("harness-e2e-local-subject", modelKey(subject));
        elements.runError.hidden = true;
        renderJob(
          await api("./api/local/run", {
            method: "POST",
            body: JSON.stringify({
              label: values.get("label"),
              url: values.get("url"),
              model: subject.model,
              provider: subject.provider,
              judge_model: judge?.model || "",
              judge_provider: judge?.provider || "",
              scenarios,
              runs: Number(values.get("runs")),
              technical_retries: Number(values.get("technical_retries")),
              seed: values.get("seed") ? Number(values.get("seed")) : null,
            }),
          }),
        );
      } catch (error) {
        elements.runError.hidden = false;
        elements.runError.textContent = error.message;
        elements.runStatus.textContent = "Could not start";
      }
    });
    elements.cancel.addEventListener("click", async () => {
      try {
        renderJob(
          await api("./api/local/run/cancel", {
            method: "POST",
            body: "{}",
          }),
        );
      } catch (error) {
        elements.runError.hidden = false;
        elements.runError.textContent = error.message;
      }
    });
    elements.catalogRefresh.addEventListener("click", refreshCatalog);
    elements.scenarioAll.addEventListener("click", () => {
      scenarioInputs().forEach((input) => {
        input.checked = true;
      });
      updateScenarioSummary();
    });
    elements.scenarioNone.addEventListener("click", () => {
      scenarioInputs().forEach((input) => {
        input.checked = false;
      });
      updateScenarioSummary();
    });
    elements.scenarioOptions.addEventListener("change", updateScenarioSummary);
    refreshJob().then(refreshCatalog);
  }

  global.HarnessLocalRunner = { initialize };
})(window);
