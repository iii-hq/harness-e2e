# PR A · ui/foundation — design-system foundation

Branch `ui/foundation` · base main (after #62 and #71 merge) · depends on #62 ui/guard-rails, #71 ui/wave-1 · size 1–2 weeks.

Read `docs/ui-migration/README.md` first: it holds the rules every UI PR follows and the verification recipe. This brief lists what this PR closes; the audit reports (`docs/ui-migration/reports/`) hold the full finding text and the screenshots each one cites.

## Scope

One PR in five ordered commits. Nothing here migrates a page; every page must render identically apart from the substituted colours.

1. **Tokens** — closed type scale (6 sizes, 2 families) in `design-system/foundations.css`; `.ds-label` is the only uppercase style; delete the neon palette from `:root` in `legacy.css (old :root palette, removed)` and move standalone tokens into `components/dashboard-shell.css`; replace the 63 `rgba()` literals in `legacy.css` with `color-mix()` over tokens; set `color-scheme` on the shell; split `--accent` from `--info`.
2. **Primitives** — `FilterChip` (with count), `DataTable` (row-link, sticky header, container-driven collapse, numeric right-aligned), `EmptyState`, `DeltaValue`, `Callout`, `Field`/`Input`/`Select` (36px, fill, edge ≥ 3:1), `PageHeader` breadcrumb. Tests in `primitives.test.tsx`, one usage story per primitive.
3. **Dialog/Sheet** — `showModal()`, focus trap, Escape handled on the element (Chromium groups a modal opened from an Escape press with its parent), fixed footer, becomes a full sheet below 720px, backdrop + opaque panel, no shadow in the console. Port `LocalRunnerDialog`, `LocalScenarioEditor`, `AssessmentDetailDialog`, `TranscriptDialog` and `DiscardDraftDialog` to it.
4. **Shell navigation** — sections as a `<nav>` of links with `aria-current`; the primary action of each section lives in the page (console header keeps context, close, theme); `scrollTo(0, 0)` on route change; one `<main>` and one skip-link in the shell; lucide icons; console title carries the open entity.
5. **Container queries** — remove `important` from the Tailwind import (`index.css (Tailwind import)`) and resolve the `@layer` conflicts it hid; `container-type: inline-size` on the root with two breakpoints (`narrow` ≤ 720, `compact` ≤ 480) replacing the 11 media queries; drop `overflow-x: hidden` from the root.

## Mockups

`Sistema.dc.html` (type scale, tokens, primitives) on the design canvas. Canvas: https://claude.ai/code/artifact/7ce1b04a-7e72-4432-82e2-9c0198379413. Roadmap: https://claude.ai/code/artifact/43057679-eab9-456b-aa6f-8393c64a92ab.

## Legacy to delete

`legacy.css (old :root palette, removed)` (neon palette), `.section-nav` remnants, every `rgba(255,120,111…)` and other literal colours, the 11 media queries in `dashboard-shell.css` and `legacy.css`.

## Findings to close

34 ids. Work P0 first inside each surface. Each line is `id (priority · category) — what is wrong → what to do`; when a recommendation names a file:line it refers to the code at audit time (2026-09-01) and may have moved.

### Plan creation

- **PN-23** (P2 · diálogo/ações) — "Discard and continue" usa `bg-brand` (mesma cor do "Create draft plan") para uma ação destrutiva; → "Keep editing" primário/secundário neutro, "Discard" em estilo quiet com texto alert; botões do DS; renomear props para `onKeepEditing`/`onDiscard`.

### Shell, design system, theme, responsive

- **A11Y-03** (P0 · tamanho) — 17 tamanhos < 11px renderizados; → Piso 11px para tudo o que se lê; 12px para dados; remover `small` aninhado em `small`.
- **A11Y-04** (P1 · alvos) — Header actions 28px (`AppHeader.tsx:23` `min-h-7`), `ds-button-compact` 28px (`primitives.css:63`), "Set A" ≈24px e "View details" ≈26px (`testhist-chess-light-1440.png`), "View metrics" ≈26px ×47, tabs ≈36px, `×` do console 36px. → Mínimo 32px + 8px de gap; 40px em `narrow`; remover `compact` de ações de linha.
- **A11Y-05** (P1 · landmarks) — `<main>` aninhado: shell + página (locais na anatomia). → Páginas → `<section aria-labelledby="page-title">`; `main` único no shell; seções em `<nav>`.
- **A11Y-06** (P1 · headings) — Overview, Executions, Tests catalog: sem h1 (primeiro heading H2 "e2e::* control-plane run"/"Recent executions"); → h1 = nome da seção/entidade em cada página (`PageHeader headingLevel=1`); h2 blocos; h3 subblocos; `<dialog>` montado só quando aberto.
- **A11Y-07** (P1 · skip links) — `ExecutionPage.tsx:628-630`: o "skip link" é "Back to executions" e navega para fora; → Um skip link no shell → `#harness-e2e-content` (com `tabindex=-1`), posicionado no topo do `PageMain`.
- **A11Y-10** (P1 · estado atual) — `aria-current` só em código morto (`SectionNav.tsx:43`) e `LocalPlanPage.tsx:1687`; → `aria-current="page"` nas seções (como links) e no último crumb.
- **DK-04** (P1 · cores hardcoded) — Os `rgba(255,120,111,…)` (salmão), `rgba(158,230,108,…)`, `rgba(255,209,102,…)`, `rgba(123,199,255,…)` de DS-03 não trocam com o tema: em dark convivem com `#f05d68/#36c98f/#f5a524/#28a8f7` do console (dois vermelhos, dois verdes, dois azuis); → `color-mix(var(--danger) 8%, transparent)` etc. — resolve os dois temas de uma vez.
- **DS-03** (P1 · cor / paleta legada) — A paleta neon `:root` ainda alimenta 63 literais `rgba` que não seguem tema nem console: salmão `rgba(255,120,111,…)` em `.detail-alert` `:2353-2355`, `.failure-chip` `:2407-2416`, `.conversation-tool-error` `:3543-3574`, `.matrix-failed` `:1970-1971`; → Substituir todos por `color-mix(in srgb, var(--danger|--success|--warning|--accent) N%, transparent)`; apagar `--code-*` e usar `--surface-raised`.
- **DS-04** (P1 · tipografia (famílias)) — A sans renderizada no console não é Inter: o stack começa por `-apple-system` em 61/61 JSON (`fonts`), porque `.harness-e2e-shell` usa `var(--font-sans, Inter…)` (`dashboard-shell.css:62`) e o host define `--font-sans`. → Duas famílias, ambas via token do host: `var(--font-sans)` e `var(--font-mono)`; remover `@fontsource/inter` se o console já serve a sans; trocar SFMono por `var(--font-mono)`.
- **DS-05** (P1 · tipografia (escala)) — 44 tamanhos computados distintos nas capturas (8px → 40px), 17 abaixo de 11px; → Escala fechada abaixo ("Mapa de tipografia") + lint que proíbe `font-size` literal e `text-[…]`.
- **DS-06** (P1 · rótulos) — 54 `text-transform: uppercase` com 10+ trackings (0.045/0.05/0.055/0.06/0.07/0.08em, -0.035em…); → Uma classe `.ds-label` (11px/0.06em/soft) e proibir uppercase fora dela.
- **DS-08** (P1 · componentes duplicados) — Stepper "01/02/03" existe em 3 versões: cartões com borda em Plans (`PlansPage.tsx:462-472`, `plans-light-1440.png`), números accent nos diálogos/plan new (`ExecutionSetup.tsx:214-325`, `dlg-runsuite-light-1440.png`) e `SectionNav` morto. → Um componente `Steps` (ou remover: em Plans o stepper é decorativo).
- **DS-09** (P1 · botões) — Quatro implementações: `.ds-button` (36px/13px), `.button` legado (19 regras; → Só `Button` do DS; primary sempre tinta; accent reservado a links/foco.
- **DS-10** (P1 · métricas) — Quatro variantes do "cartão de métrica": `MetricCard` (fill), `.latest-kpi` (fill, Overview), `.plan-*` KPIs com borda (`index.css (Tailwind import)363`, `plans-light-1440.png` "NEEDS ACTION 6"), exec detail com borda (`exec-failed-light-1440.png`), `tmh-*`… → `MetricCard` único com `tone` e `size`.
- **RD-03** (P1 · tabelas) — A 720 as tabelas mostram 3 colunas e cortam a 4ª a meio: `executions-light-720.png` (EXECUTION / RESULT / "openai-codex/codex/gpt-" — SCOPE, OUTCOME, EFFICIENCY, EVIDENCE fora), `tests-light-720.png` (TEST / VERSION / LIFECYCLE / COMPLEXITY / "HUMAN HO"); → Regras gerais abaixo: colapsar colunas secundárias em metadados sob o nome (≤720) e em cartão (≤480); ação de linha sempre visível (coluna fixa à direita ou no cartão).
- **S-04** (P1 · ações / descobribilidade) — As ações do header mudam por página sem padrão visível: Overview/Executions = "new plan", "quick run" (`OverviewPage.tsx:672-681`); → Uma ação primária por seção no header (Overview/Executions: "run"; Tests: "+ new test"; Plans: "+ new plan"), em estilo tinta; secundárias ("compare", "new plan" fora de Plans) vão para o conteúdo.
- **S-05** (P1 · semântica / navegação) — As seções são `<button role="tab">` que mudam o hash (`DashboardShell.tsx:216-218, 270-282`): não são links (sem abrir em nova aba, sem copiar URL, sem `aria-current`); → `<nav aria-label="Harness E2E sections">` com `<a href={hashForSection()} aria-current="page">`, mantendo o visual de tabs do console.
- **S-07** (P1 · UX / estado) — Troca de página em standalone = `window.location.reload()` (`use-hash-route.ts:204-210`): flash (html pinta `#0a0c0b` antes do React, `index.css:38, 82-93`), perda de filtros/scroll/diálogos, re-download do bundle. → Remover o reload (as páginas já são componentes React); no `hashchange` fazer `mainRef.current.scrollTo(0,0)` salvo quando há anchor; persistir filtros na query (`?status=failed`).
- **S-09** (P1 · consistência / "onde estou") — Quatro padrões de localização/voltar: eyebrow + h1 + footer "Back to overview" (Plans, `PlansPage.tsx:611-613`), breadcrumb "Overview / Executions / …" + footer "Back to all executions" (`ExecutionPage.tsx:708-716, 908-914`), breadcrumb "Tests /… → Um único padrão: breadcrumb DS (contexto) + h1 da entidade; remover footers "Back to…" (a tab já é o voltar).
- **A11Y-11** (P2 · uppercase 11px) — Uppercase + tracking em 11px mono com cor muted é a pior combinação do app para baixa visão/dislexia (todas as th, eyebrows, rótulos de métrica). → 11px só com `--text-soft`; considerar lowercase mono (já é o vocabulário) e reservar uppercase a th.
- **DK-05** (P2 · semântica de matiz) — O accent muda de laranja queimado (light) para azul (dark): o mesmo elemento ("EXECUTION SETUP", ponto do eyebrow "LOCAL COMPARISON WORKSPACE", "Browse tests →", badge LOCAL, números do stepper) muda de "quente/ação" para "frio/info". → Tokens separados `--accent` (marca) e `--info` (running/links informativos), ambos do host; não usar accent para rótulos decorativos.
- **DK-06** (P2 · standalone) — `html, body { background: var(--bg) }` com `--bg: #0a0c0b` (`index.css:38, 82-93`) independentemente do tema; → Mover `--bg` claro/escuro para `:root[data-theme]` ou pintar `html` com `var(--color-bg)` do shell.
- **DK-07** (P2 · alerta) — Banner "Judge model needs investigation" usa `color-mix(var(--danger) 7%)` (`OverviewPage.tsx:351`) → rosa em light, vinho em dark (`crop-ov-dark-hero.png`) — correto, mas é o único alerta com esse tratamento; → Um `Callout tone="danger|warning|info"` no DS.
- **DK-08** (P2 · `color-scheme`) — `:root { color-scheme: dark }` (`index.css:37`) é a base; → Declarar `color-scheme` no `.harness-e2e-shell[data-theme]` (`dashboard-shell.css`), não em `:root`.
- **DS-11** (P2 · headings) — `:where(h1,h2,h3,h4)` força mono lowercase (`dashboard-shell.css:121-126`) mas páginas sobrescrevem: "Execution history" (sans, `plan-sysprompt-light-1440.png`), "Create a quick benchmark result" (sans bold, `dlg-runsuite-light-1440.png`), "Judge model needs… → h1/h2 = mono lowercase 1rem; h3+ = sans 0.875rem semibold; nada mais.
- **DS-12** (P2 · breakpoints) — 11 media queries diferentes em `legacy.css` (1120, 960, 900, 840, 760, 720, 620, 560, 500, 430) + Tailwind `sm/md/lg/xl` (64/29/61/2 usos) + `max-[560px]` ×17, `max-[640px]`, `max-[460px]`, `max-[390px]` — todos por viewport — enquanto o shell decide `narrow`… → `@container` queries (`container-type: inline-size` no `.harness-e2e-dashboard`) com 2 pontos: `narrow` (≤720) e `compact` (≤480); propagar `data-narrow` para as páginas.
- **DS-13** (P2 · sombras) — `--shadow:none` (`dashboard-shell.css:55`) mas diálogos usam `0 32px 100px rgba(0,0,0,.65)` (`index.css:1498, 3151`) e há `inset 3px` como "borda" (`:3545`). → `--shadow-dialog: 0 24px 64px rgb(0 0 0/.25)` único; `inset` → `border-left`.
- **DS-14** (P2 · build) — `@import "tailwindcss" important` (`index.css (Tailwind import)`) torna toda utility `!important`: causa raiz de S-01, impede overrides por tema/estado e força mais `!important` no legado (`:7589-7603`). → Remover `important`; resolver conflitos com `@layer base/components/utilities` e especificidade normal.
- **RD-10** (P2 · Plan new / diálogos) — A 390 o painel "review setup" + CTA "Create draft plan" vem depois da lista de 45 testes (`plan-new-light-390.png`, CTA a ~3.000px); → ≤720: diálogo = sheet 100% com barra de ações sticky no rodapé (CTA + Cancel).
- **RD-11** (P2 · raiz) — `overflow-x: hidden` na raiz (`DashboardShell.tsx:249`, `console-overrides.css:18`) + `min-width: 320px`: em painéis do console < 320px o conteúdo é cortado sem scroll; → Remover `overflow-x-hidden` da raiz; cada tabela/gráfico gere o seu `overflow-x:auto` com `scroll-shadow`.
- **S-11** (P2 · iconografia) — Ícones das tabs desenhados à mão (`DashboardShell.tsx:109-144`: frasco para Tests, relógio para Executions, "nós" para Plans, relógio de novo para Coverage) enquanto o resto usa lucide (`ThemeToggle.tsx:1`, `LegacyLoadError.tsx:1`). → Usar lucide (`LayoutDashboard`, `FlaskConical`, `History`, `GitCompare`) ou nenhum ícone.
- **S-14** (P2 · título do documento) — `App.tsx:56-69` gere `document.title` por rota, mas embutido o título é sempre "iii console" (todos os JSON) → histórico do browser sem distinção. → Expor o título por API do host (se existir) ou aceitar e documentar.
- **RD-12** (— · referência positiva) — Test history (`testhist-chess-light-720.png`, `…-390-part1.png`): filtros 5→2→1 colunas, KPIs 5→2→1, tabela vira cartão com rótulos — é o modelo a generalizar. → Extrair `.history-table` (`index.css:9459+`) para a `Table` do DS.

## Acceptance

- Screenshots of every route at 1440/720/390 in both themes are identical to the baseline apart from substituted colours.
- No `rgba(255, 120, 111` (or any hex/rgba literal outside tokens) left in CSS; `tests/dashboard/theme-contrast.test.cjs` green.
- One primary action per section; unique landmarks (axe); no "Back to…" footer on any route.
- `dist-console` builds and the console sanitizer strips nothing new (grep the built CSS for `box-shadow` and hex literals).
- Every primitive has a test that renders it in a realistic story; no page migrated yet.

## PR body

Follow the definition of done in the README: what changed per surface with the ids closed, the ratchet numbers before/after, the verification commands run, and a live-evidence table from the capture script (both widths, both themes). Say plainly what was not exercised live.
