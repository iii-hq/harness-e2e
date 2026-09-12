# Reestruturação do Harness E2E

Organização Git após a separação de 11/09: a branch `feat/console-metrics-ui`
mantém a reestruturação interna do E2E e o contrato de sessões anterior. O novo
contrato compartilhado de escopo, correlação e herança fica em
`feat/e2e-session-integration`, nos repositórios `harness-e2e` e `workers`.
A interface de transcritos retidos permanece no E2E. A ativação do isolamento
exige reunir as duas branches de integração e fazer o corte coordenado da
seção 13. Os registros históricos de validação das etapas abaixo descrevem o
conjunto completo antes desta separação.

O recorte do E2E está no commit `49718cb`, disponível em
`/home/layon/workspaces/harness-e2e-session-integration`. O recorte do Workers
está no commit `f00c40c77`, disponível em
`/home/layon/workspaces/workers-e2e-session-integration`.
As alterações movidas já foram retiradas dos checkouts de origem, preservando
os demais arquivos locais. O backup e os manifestos de hashes estão em
`/tmp/e2e-integration-split-rq3xb3f3`.

A recomposição dos seis arquivos Rust é automática e idêntica ao estado
anterior. A fixture `tests/golden/schemas/harness.send.json` exige resolver um
conflito entre a simplificação local do snapshot e a definição `SessionScope`:
manter o snapshot simplificado, acrescentar `SessionScope` e tornar
`SessionInit.scope` obrigatório. A versão recomposta e o patch verificados
estão preservados em `/tmp/e2e-session-contract-split-mzZHvE`.

Após a separação, os testes de biblioteca do E2E passaram: 687 aprovados e
1 ignorado por exigir fixture externa. O recorte de integração do E2E passou
em `cargo check --lib` e nos dois testes focados de contrato e payload SWE.
Os cinco testes da migração de sessões passaram no recorte do Workers.

Data: 11 de setembro de 2026. Decisão: hard refactor autorizado. Diagnóstico, estabilização de dados, isolamento de sessões, separação do domínio de planos e migrações explícitas implementados e validados localmente. O fluxo visual de referência de plano, execução e comparação foi implementado na etapa 4 (seção 14). A propagação para as demais telas e a retirada do produto standalone foram implementadas na etapa 5 (seção 15). A seção 13 registra o contrato atual e o estado de implantação; a seção 12 conserva o histórico da etapa anterior.

## 1. Direção aprovada

O Harness E2E deve ser o espaço do Console para configurar avaliações, acompanhar execuções, comparar resultados e investigar evidências. As conversas produzidas pelos testes pertencem a esse espaço e não devem ocupar o histórico de conversas pessoais.

A mudança visual é uma etapa de uma reestruturação maior. A ordem recomendada é corrigir as inconsistências de dados identificadas, estabelecer o isolamento dos chats, consolidar os contratos usados pelas telas e então aplicar a nova organização visual. A implementação pode aproveitar o trabalho existente; não há necessidade demonstrada de outro serviço de chats, outro design system ou uma divisão do worker em vários serviços.

A estratégia é **hard refactor**: um contrato de destino, atualização coordenada dos produtores e consumidores, migração explícita dos dados anteriores e retirada dos caminhos substituídos. Não haverá aliases de API antigos, desserialização que invente classificação nem execução dupla das duas arquiteturas. A entrega continua dividida em etapas verificáveis; isso não implica retrocompatibilidade. CLI, CI e Release Control são consumidores que precisam funcionar no contrato vigente, não motivos para manter contratos obsoletos.

As prioridades são:

1. **Confiança nos resultados:** lista, detalhe, histórico e comparação devem representar a mesma execução e declarar os limites da evidência.
2. **Separação das conversas:** sessões automáticas E2E, incluindo filhos e tentativas, ficam fora da descoberta normal de chats.
3. **Responsabilidades claras:** execução, planos, resultados e apresentação têm donos definidos, com uma fonte autoritativa por tipo de dado.
4. **Interface consistente:** métricas agrupadas e por teste primeiro; investigação acessível sem dominar a visão inicial.
5. **Menos manutenção duplicada:** reaproveitar componentes e contratos existentes e retirar os caminhos que deixam de ter função.

## 2. Base e limites da análise

A tarefa anterior, [Planejar visual integrado ao Console](thread://01a0889c-0340-79d0-9233-c344edbab4cd?hostId=remote-ssh-discovered%3Aubuntu-iii), foi consultada diretamente. As decisões abaixo foram recuperadas e conferidas no código atual:

- Console como interface principal.
- Input e output de tokens, parcelas normal/cache, turnos, function calls/errors, tempo, nota e gasto como métricas principais.
- Plano e comparação começam pelo agrupado e seguem para os mesmos dados por teste.
- Nota agrupada com peso igual por teste; repetições não aumentam o peso do teste.
- Ausência, zero e observação parcial são distintos.
- O filtro opcional de nota zero/ausente retira o teste dos dois lados e recalcula resumo, tabela e deltas.
- Juiz e critérios booleanos não orientam a nova experiência principal. O código ainda contém avaliadores e contratos específicos; sua eventual remoção exige verificar os cenários consumidores.

O [contrato de métricas existente](design/console-metrics-v1.md) continua sendo referência. Seu registro de validações anteriores não foi tratado como validação desta auditoria.

Foram analisados o checkout `feat/console-metrics-ui`, HEAD `801e7e62e7b509885b133dc50c4cc4777816eb14`, e as alterações locais presentes. O checkout inclui tanto a primeira versão visual quanto mudanças posteriores de persistência e controle. `src/persistence.rs` e `worker-compose.control.yaml` estavam entre os arquivos ainda não rastreados. Os achados identificados como **WIP** descrevem esse estado local, não a release publicada.

A integração de chats foi rastreada no checkout Workers, branch `main`, HEAD `093ea0f8195a9ca6ada30e13adba19ca75ab5b96`, também com alterações locais preservadas. Foi consultada a [proposta arquitetural anterior](/home/layon/workspaces/workers/tech-specs/2026-08-harness-e2e/README.md), ainda não rastreada naquele checkout, para contextualizar fronteiras iii, evidência e retries. Ela não comprova implementação; a prioridade de métricas desta proposta segue as decisões mais recentes do usuário.

A auditoria visual usou o Console em `http://127.0.0.1:3113/#/ext/harness-e2e/`. O bundle servido contém a nova visão de métricas, mas sua equivalência byte a byte com o checkout não foi verificada. A coincidência entre um sintoma visual e um caminho de código é evidência de investigação, não prova da revisão carregada no processo.

A etapa 0 alterou apenas documentação e preservou capturas. A implementação e a validação local da etapa 1 estão registradas na seção 12; a auditoria inicial abaixo permanece como diagnóstico do estado anterior.

A revisão foi dirigida aos fluxos e contratos estruturais. Não é uma revisão linha a linha de todos os cenários/avaliadores nem uma auditoria de segurança completa.

## 3. Estrutura atual e pontos a preservar

| Responsabilidade | Implementação atual | Fronteira que deve continuar clara |
|---|---|---|
| Inicialização e integração | `src/worker.rs`, `src/manifest.rs`, `src/console_ui.rs` | Compose fornece ambiente; worker registra funções e assets do Console |
| Admissão e execução | `src/control.rs`, `src/suite.rs`, `src/workflow/` | Idempotência, execução, cancelamento e estado operacional pertencem ao worker |
| Definições e avaliação | `src/scenarios/`, `scenarios/`, `src/markdown/`, `src/test_plan.rs` | Contratos versionados, inputs e avaliadores determinam o significado do resultado |
| Evidência e recuperação | `src/journal.rs`, `src/report.rs`, `src/artifact.rs`, `src/durable.rs` | Journal registra transições; bundle nativo conserva evidência completa |
| Persistência operacional, WIP | `src/persistence.rs` | SQL contém estado e projeções; o database worker detém acesso ao banco |
| Planos salvos e composição | `src/plans/store.rs` | Hoje persiste plano e execução composta em arquivos; coordena filhos nativos |
| APIs de apresentação | `src/dashboard/{controller,bus,api,read_model,presenter}.rs` | Leitura e projeção para UI, sem inventar outro resultado de execução |
| Interface | `dashboard/src/console-entry.tsx`, `App.tsx`, páginas, componentes e projeções | Console fornece host, transporte, tema e navegação de painel |
| Campanhas remotas | `.github/workflows/exact-stack-e2e.yml`, `scripts/` | Release Control despacha; este repositório materializa escopo, stack exata e evidência |
| Sessões e chat | Workers: Harness, Session Manager e Console | Sessão executada e conversa selecionada pelo usuário são conceitos separados |

A separação entre SQL, journal e bundle pode ser correta. O requisito é explicitar o que cada um garante e como recuperar divergências; reuni-los em um único armazenamento não é objetivo desta proposta.

Devem ser preservados: admissão idempotente antes do dispatch, correlação execução/run/tentativa, journal verificável, artefatos nativos, separação entre falha técnica e qualidade, compensação/cancelamento de trabalho ativo e identidade exata de avaliação. O restart atual interrompe e reconcilia trabalho ativo; não retoma automaticamente a execução do modelo ([controle](../src/control.rs#L1564)).

## 4. Análise crítica do código

Prioridade **P1** significa impacto no fluxo principal, confiança ou isolamento das conversas. **P2** significa manutenção, clareza ou usabilidade. Riscos sem reprodução são identificados como tais.

### C1 — A origem E2E não participa da descoberta de conversas — P1

O E2E já fornece metadata de origem, por exemplo `e2e_run_id` e `e2e_scenario` em [suite.rs](../src/suite.rs#L4418). Contudo, o Console pede apenas as 200 sessões mais recentes sem filtro e ignora o próximo cursor. As assinaturas de eventos também abrangem todas as sessões.

Isso permite tanto poluição visual quanto deslocamento de conversas pessoais para fora da página carregada. Filtrar depois da paginação não recupera essas vagas. A criação provisória de uma conversa antes de buscar sua metadata também impede resolver o problema apenas escondendo linhas depois do carregamento.

Evidência Workers: [consulta de sessões](/home/layon/workspaces/workers/console/web/src/lib/sessions/api.ts:36), [assinaturas](/home/layon/workspaces/workers/console/web/src/lib/sessions/events.ts:69), [inserção provisória](/home/layon/workspaces/workers/console/web/src/hooks/use-conversations.ts:1733). A proposta completa está na seção 6.

### C2 — Lista e detalhe usam dados de completude diferentes, WIP — P1

O novo [registro SQL](../src/persistence.rs#L302) remove intencionalmente `report`, `manifest` e `observation`. [ControlPlane::records](../src/control.rs#L847) retorna esses registros sem hidratar os bundles. O [read model](../src/dashboard/read_model.rs#L395) repassa `record.report`, e [index_run](../src/dashboard/read_model.rs#L453) abandona a indexação quando ele está ausente.

Já o detalhe individual passa por [hydrate_native_evidence](../src/control.rs#L1797), que lê o report e o manifest nativos. Assim, o caminho do WIP permite um detalhe com evidência enquanto o modelo de lista/histórico/coortes não a indexa. A auditoria visual encontrou um sintoma compatível: “no report retained” na lista, com evidência disponível no detalhe. A revisão do binário servido ainda precisa ser correlacionada para afirmar que é a mesma causa em runtime.

**Mudança proposta:** manter a listagem compacta e fazer as projeções de disponibilidade, histórico e coortes consumirem as fontes corretas. Preferir as projeções de runs já persistidas quando tiverem o contrato necessário; acessar bundles sob demanda onde a evidência completa for exigida. Não restaurar uma leitura de todos os transcritos a cada abertura do Overview.

**Aceite:** a mesma execução conserva estado, disponibilidade, histórico e identidade antes/depois de restart. Ausência do bundle não deve apagar a existência da execução nem ser confundida com ausência de qualquer resultado.

### C3 — Execuções compostas de plano deixam de participar da consulta, WIP — P1

[execution_summaries](../src/dashboard/controller.rs#L166) retorna imediatamente quando há ControlPlane, antes de juntar os pais e vínculos do PlanStore. [execution_bundle](../src/dashboard/bus.rs#L702) exige encontrar o ID nessa listagem antes de consultar o detalhe do plano.

Para um pai armazenado apenas pelo PlanStore, o caminho pode retornar `execution not found` antes de ler um detalhe existente. Na auditoria visual, o plano Software engineering apresentou exatamente essa mensagem ao selecionar sua execução. Isso não comprova que os arquivos daquele ID existem; o defeito de roteamento do WIP é verificável independentemente dessa disponibilidade.

**Mudança proposta:** recompor pais/filhos na projeção ou resolver diretamente o tipo de execução antes de negar sua existência. Preservar os IDs e a distinção entre execução composta e execução nativa. Não criar um Results sintético para fazer a página funcionar.

**Aceite:** execução do plano acessível pela lista, seleção de baseline/candidato e link direto; filhos nativos continuam acessíveis sem duplicar os totais.

### C4 — A migração de persistência ainda não corresponde ao contrato documentado — P1

O [schema SQL](../src/persistence.rs#L21) cria tabelas de planos, execuções compostas e slots. Entretanto, o [PlanStore](../src/plans/store.rs) lê e escreve esses objetos em `plan-store/plans` e `plan-store/executions`, sem usar Persistence. O [README local](../README.md#L286) já afirma que planos são gravados por `database::*`.

Isso comprova uma migração incompleta e documentação adiantada em relação ao código; não comprova perda de dados. O compose com `path://../workers/database` é configuração local, e não há evidência suficiente nesta auditoria para atribuir esse caminho ao pacote publicado.

**Decisão proposta para o primeiro lote:** manter planos autoritativos no PlanStore atual e tornar esse limite explícito. Não ampliar tabelas sem consumidor nem introduzir escrita dupla. Se a persistência SQL de planos for objetivo do trabalho paralelo, concluí-la em uma mudança própria, com migração, retomada de leitura e testes de recuperação antes de mudar o contrato público.

### C5 — O contrato de métricas ainda não é uniforme nas superfícies — P1

A primeira versão já centraliza média por teste, disponibilidade e filtro em [primary-metrics.ts](../dashboard/src/lib/primary-metrics.ts#L89), usada por ExecutionPage e PlanDetailPage. Há outros caminhos ativos: [execution-metrics.ts](../dashboard/src/lib/execution-metrics.ts#L136) calcula medianas diagnósticas; o [detalhe RC](../dashboard/src/pages/ReleaseControlPlanDetailPage.tsx#L340) monta métricas com `buildPlanComparison`, `objectiveScore` e uma projeção própria.

Mediana diagnóstica e média principal podem coexistir quando seus nomes e populações são explícitos. O problema é permitir que cada página redefina “nota”, tokens, custo e recorte. Na referência RC, [objectiveScore](../dashboard/src/lib/release-control-reference.ts#L545) faz média dos runs observados, dando mais peso a testes com mais runs. Exemplo ilustrativo: um teste com dez notas 100 e outro com uma nota 0 produzem 90,91 por run, contra 50 com peso igual por teste. A projeção `RcRun` atual expõe total de tokens, sem declarar as parcelas input/output/cache da nova visão. Também há filtros em granularidades diferentes: o componente principal filtra testes agregados, enquanto a referência RC alinha slots de execução.

**Mudança proposta:** estabelecer uma projeção comum de apresentação a partir dos dados nativos e da referência RC, reaproveitando as funções existentes. Não converter referências remotas em reports nativos fictícios. O contrato deve declarar população, unidade, origem, completude e regra de pareamento, inclusive repetições. Parcelas que a referência não fornecer continuam indisponíveis.

**Aceite:** resumo e tabela usam o mesmo recorte; média principal tem peso igual por teste; repetições e slots não são confundidos; deltas incompatíveis ficam indisponíveis; diagnósticos não são apresentados como outro valor da mesma métrica.

### C6 — A adaptação visual mantém mais de uma base de componentes — P2

O [shell](../dashboard/src/components/DashboardShell.tsx) usa componentes públicos do Console, mas há também [primitivas próprias](../dashboard/src/design-system/primitives.tsx), `legacy.css`, tokens locais, overrides e um [build de 434 linhas](../dashboard/vite.console.config.ts) que remove fontes, converte cores e reescreve seletores. O [build Rust](../build.rs#L64) ainda exige os bundles standalone e Console.

O problema não é o tamanho isolado dos arquivos: as mesmas decisões visuais são mantidas em várias camadas. Da mesma forma, PlanDetailPage reúne carregamento, comparação, seleção e composição visual; movê-la inteira para outra pasta não simplificaria essas responsabilidades.

**Mudança proposta:** adotar os componentes/tokens públicos do host e manter apenas componentes de domínio E2E. Remover substituições e estilos à medida que seus consumidores forem migrados. Retirar a transformação de cores/fontes quando as fontes já obedecerem ao contrato do Console; preservar o isolamento de CSS enquanto necessário. A exclusão do standalone precisa acompanhar build, testes e consumidores de artefatos, preservando CLI e execução em CI.

### C7 — A validação existente precisa atravessar as novas fronteiras — P1/P2

O projeto possui testes Rust, TypeScript, Python, verificação de contratos e fluxos de navegador no [CI](../.github/workflows/ci.yml). Isso é uma base a preservar. Porém, testes de projeções isoladas não garantem que a nova persistência alimenta a mesma projeção nem que sessões E2E deixam de entrar no diretório do Console.

Nesta auditoria passaram TypeScript e 38 testes de quatro módulos: métricas principais, comparação de plano, data source e chats de cenário. Isso valida esse recorte, não o backend, a suíte completa ou o isolamento proposto. O [script de capturas](../dashboard/scripts/screenshots.mjs#L29) ainda inclui `coverage`, que não aparece no [parser atual](../dashboard/src/hooks/use-hash-route.ts#L108); a documentação de rotas também precisa acompanhar o produto efetivo.

## 5. Auditoria das telas

Os números abaixo pertencem ao ambiente observado em 11/09, não são constantes do produto. Nem todas as diferenças entre telas são erros de cálculo: algumas usam universos distintos sem explicar essa diferença.

| Tela | Papel atual | Achado observado | Mudança proposta |
|---|---|---|---|
| Overview | Sinal operacional e acesso ao recente | Ênfase em pass rate/coverage/runtime/tokens; última execução incompleta e dados ausentes | Resumo operacional compacto com a mesma semântica de estado das execuções; avaliar fusão com a lista |
| Execuções | Histórico | Execução `d9c9ba46…00185a` aparece `incomplete / no report retained`; detalhe tem evidência | Corrigir disponibilidade e projeção; explicitar o que não foi retido |
| Detalhe da execução | Métricas agrupadas e por teste | Nova visão de métricas está servida; cabeçalho `failed`, cenário `inconclusive`, evidência `Infrastructure Error` | Nomear ciclo de execução, validade técnica e resultado do teste separadamente |
| Evidência de tentativa | Explicar o resultado | Runtime Kanban obrigatório ausente; recomendação aponta coleta/serialização | Orientação baseada na causa observada, sem atribuir genericamente falha ao coletor |
| Evidência de tentativa | Navegação e teclado | Modal abre sem mudar URL; Back sai da execução; Escape fecha e foco retorna ao BODY | Usar a rota de tentativa já prevista; restaurar foco ao acionador |
| Testes | Catálogo e disponibilidade | Overview mostra 62; título Tests mostra 52; rodapé 50 de 62 carregados; criação oferece 67 | Distinguir total do catálogo, carregados, locais, executáveis e filtrados |
| Histórico do teste | Contrato e resultados | `never run` explícito; prompt aberto domina a página sem histórico | Separar Contrato e Histórico e manter estado vazio direto |
| Comparação de versões | A/B dentro de coorte | Sem coortes avaliadas; controles indisponíveis e explicação visível | Reaproveitar experiência de comparação no contexto de execução/plano/teste |
| Planos | Configurações e execuções | 35 planos, 34 `needs action`; nomes repetidos e descrições extensas | Estado e última execução legíveis; distinguir origem local/RC/template sem novos tipos de plano |
| Detalhe de plano | Baseline e candidatos | Seleção por IDs brutos; `execution metrics unavailable / execution not found` | Corrigir resolução; identificar opções por modelo/data/estado com ID secundário |
| Novo plano | Configuração | Estrutura de formulário compreensível; rodapé ocupa cerca de 130px em painel de 600px; endpoint técnico no resumo | Resumo compacto e opções técnicas secundárias; manter ações acessíveis |
| Referências RC | Consulta e comparação remota | Bridge indisponível, com mensagem explícita sobre Engine | Preservar erro explícito e operação local; auditar detalhe RC quando conectado |

Fontes de problemas específicos: [contagem do catálogo](../dashboard/src/pages/TestsCatalogPage.tsx#L913), [abertura do modal por estado local](../dashboard/src/components/AssessmentWorkspace.tsx#L899), [recomendação genérica de infraestrutura](../dashboard/src/lib/assessment-view.ts#L166).

Capturas preservadas no repositório:

- [Lista de execuções](design/restructure-audit-2026-09-11/executions-desktop.png) e [detalhe da execução](design/restructure-audit-2026-09-11/execution-desktop.png).
- [Evidência e causa de infraestrutura](design/restructure-audit-2026-09-11/run-evidence-desktop.png).
- [Plano com referência indisponível](design/restructure-audit-2026-09-11/plan-desktop.png).
- [Catálogo e contagens](design/restructure-audit-2026-09-11/tests-desktop.png).
- [Criação em painel estreito](design/restructure-audit-2026-09-11/new-plan-narrow.png).

O [manifesto das capturas](design/restructure-audit-2026-09-11/manifest.json) registra origem, revisões dos checkouts e hashes das capturas e dos arquivos centrais analisados. Os hashes identificam o WIP, mas seu conteúdo não foi arquivado neste documento. Os scripts, DOMs e demais capturas da sessão estão em `/tmp/harness-e2e-restructure-audit-20260911`; esse diretório é temporário.

Foram inspecionados desktop 1500×1000, painel de 600×900 e amostras de 390×844, em temas claro/escuro. Não houve overflow global nas amostras medidas; tabelas usam scroll local. Skip link e seleção de seção por teclado funcionaram. Não houve erros de console/pageerror nas verificações concluídas. Erros iniciais de seletores dos scripts foram corrigidos e não são falhas do produto.

Não foram validados ao vivo: execução ativa, sucesso com métricas completas, comparação A/B válida, loading prolongado, detalhe RC e submissão/validação do formulário. Esses estados precisam integrar a próxima validação visual.

## 6. Isolamento dos chats E2E

### 6.1 Resultado esperado

Durante uma execução, o usuário continua vendo suas conversas habituais. Sessões de teste, planners, retries e subagentes permanecem acessíveis a partir da execução e tentativa correspondentes. Inspecionar uma evidência não troca a conversa pessoal selecionada nem habilita o envio de mensagens para a sessão avaliada.

Isso é isolamento de organização e descoberta. Não cria uma fronteira de autorização e não substitui o isolamento de filesystem, ferramentas, credenciais ou recursos do executor.

### 6.2 O que existe e o que falta

| Já existe | Lacuna atual |
|---|---|
| Metadata na criação de sessões | Marcação desigual entre execução convencional, Markdown, workflows, planner e SWE |
| Filtragem por igualdade de metadata antes da paginação | Falta classificação explícita de toda sessão e seleção por escopo |
| Filtros de metadata nos eventos do Session Manager | Console usa assinaturas globais para o diretório |
| Vínculos parent_session_id/turn/function nos filhos | Filhos não herdam a classificação E2E |
| ID de sessão no run e na tentativa | Botão Chat seleciona a sessão no chat global |
| Snapshot de transcrito e TranscriptDialog | Cobertura de filhos na evidência é limitada; sessão viva e snapshot precisam ser distinguidos |

O filtro existente está em [Session Manager list](/home/layon/workspaces/workers/session-manager/src/service.rs:275), com [semântica de igualdade](/home/layon/workspaces/workers/session-manager/src/types.rs:16). O emissor também possui metadata para filtrar eventos, embora o evento público de criação não a inclua. Os [filhos do Harness](/home/layon/workspaces/workers/harness/src/subagent.rs:607) recebem vínculos, sem essa herança de origem.

### 6.3 Contrato de destino

A classificação é um campo próprio, imutável, da sessão:

```json
{
  "scope": "e2e",
  "metadata": {
    "e2e_execution_id": "<execução>",
    "e2e_run_id": "<run>",
    "e2e_attempt_id": "<tentativa>"
  }
}
```

Os IDs de correlação são preenchidos quando conhecidos; os campos de cenário/workflow/step já existentes permanecem como correlação. `scope` é obrigatório no registro persistido e na criação; não pode ser alterado por `set-meta`. Os valores armazenados são `conversation` e `e2e`.

1. Todos os produtores automáticos E2E aplicam a classificação **na criação**. Não inferir pelo título. Se o `session_id` já existir, só permitir o turno quando `scope = e2e` e a identidade de correlação esperada coincidirem. Sessão sem classificação, humana ou vinculada a outra execução/tentativa bloqueia o dispatch antes de gerar mensagens. Reuso de legado exige classificação prévia comprovada; uma nova tentativa pode receber outro ID, sem fallback silencioso. A verificação integra a operação atômica de criação/reuso da sessão: uma leitura prévia seguida de envio desprotegido não basta.
2. Harness propaga a classificação aos descendentes e conserva os vínculos existentes. Copiar apenas os campos necessários, sem copiar indiscriminadamente configuração/perfil/metadata do turno.
3. Listagem e assinaturas do diretório aceitam uma seleção explícita, por exemplo `scope: conversation | e2e | all`. O Console usa `conversation`; E2E e investigação podem selecionar `e2e` ou IDs exatos. A ausência do parâmetro é erro de contrato. Consumidores operacionais que enumeram toda a árvore pedem `all` explicitamente e são atualizados no mesmo conjunto de mudanças.
4. O mesmo predicado opera antes da paginação e antes da entrega dos eventos. A coleção da sidebar/busca contém somente conversas permitidas nesse escopo; buscar uma sessão por ID para inspeção não a insere automaticamente nessa coleção.
5. Uma sessão classificada E2E permanece assim ao longo da execução. Reconexão, atualização de metadata e descendentes não podem fazê-la reaparecer como conversa usual.

Registros antigos precisam ser classificados pela migração antes do novo leitor. A preservação das conversas acontece nessa conversão explícita, sem fallback permanente para escopo ausente. Atribuição E2E exige evidência estruturada ou descendência comprovada.

### 6.4 Acesso à evidência e eventual análise assistida

A ação principal proposta é **Ver transcrito**, usando o [visualizador existente](../dashboard/src/components/TranscriptDialog.tsx#L63) no contexto de execução/run/tentativa. O [botão atual](../dashboard/src/console-entry.tsx#L61) chama `host.chat.selectConversation`; esse acoplamento deve sair do fluxo de inspeção.

O transcrito retido é evidência do momento da avaliação. Uma sessão ainda disponível no Harness é uma fonte viva e deve ser identificada como tal. Onde a captura de filhos estiver ausente, informar a lacuna; o [report atual](../src/report.rs#L1205) já declara limitações da evidência por função dos filhos. Não prometer um replay completo a partir de dados que não foram retidos.

Como evolução opcional, **Analisar em novo chat** pode criar uma conversa pessoal com referência à execução e às evidências escolhidas. Isso não é requisito do primeiro lote de isolamento. Caso entre no escopo, usa um novo ID e nunca envia à sessão avaliada. O fork atual copia a metadata inteira ([Session Manager](/home/layon/workspaces/workers/session-manager/src/service.rs:1377)); portanto, usá-lo diretamente também copiaria a classificação E2E.

### 6.5 Histórico e migração

Classificar sessões existentes apenas por metadata E2E estruturada, IDs exatos de reports/tentativas e vínculos duráveis dos descendentes. Workflows precisam ser reconhecidos por suas próprias correlações, pois nem todos possuem `e2e_scenario`. Título/prefixo não é critério confiável.

A migração deve produzir um manifesto revisável de IDs, justificativa e metadata anterior. O [set-meta atual](/home/layon/workspaces/workers/session-manager/src/service.rs:332) substitui a metadata inteira, altera `updated_at` e emite evento. Uma atualização ingênua pode sobrescrever metadata concorrente e reordenar conversas. O procedimento precisa preservar os demais campos, controlar concorrência e renovar os caches/assinaturas do Console. Todas as sessões recebem classificação na migração: evidência E2E estruturada ou herança comprovada resulta em `e2e`; as demais ficam em `conversation`. Pais ausentes que impeçam uma decisão e correlações conflitantes bloqueiam o apply. O leitor novo recusa registros sem escopo. Nenhuma conversa é apagada para resolver poluição. A migração roda com os produtores parados, preserva mensagens e tempos e exige backup; não usa o endpoint `set-meta`.

### 6.6 Mudanças por repositório e alternativas

| Local | Mudança concentrada |
|---|---|
| harness-e2e | Marcação consistente nos produtores; correlação; navegação para transcritos de execução/tentativa |
| workers/harness | Herança da classificação na criação de filhos; dispatch condicionado à identidade esperada da sessão |
| workers/session-manager | Seleção consistente em consultas/eventos; verificação atômica na criação/reuso e apoio à classificação histórica |
| workers/console | Diretório e busca no escopo de conversas; inspeção separada da seleção global; atualização de caches |
| workers/claude-code, pi, kanban, cron, eval, security-scan | Atualizar criação, herança e listagem para o contrato explícito, sem manter chamadas antigas |

| Alternativa | Avaliação |
|---|---|
| Esconder linhas somente na sidebar | Não resolve paginação, eventos, aparições transitórias ou filhos órfãos |
| Isolamento lógico no Session Manager atual | Recomendado; reutiliza metadata, filtros e armazenamento existentes |
| Outro storage ou instância | Exige uma necessidade adicional de segurança, retenção ou capacidade ainda não demonstrada |

O [armazenamento de sessões em arquivos](/home/layon/workspaces/workers/session-manager/src/store/fs.rs:370) enumera metadata antes da filtragem. O isolamento lógico resolve o conteúdo da listagem, mas não elimina esse custo. Medir volume/latência antes de introduzir índice ou particionamento.

## 7. Nova organização de produto

Proposta de navegação principal: **Execuções · Planos · Testes**. O resumo útil do Overview passa para o início de Execuções, evitando duas entradas para o mesmo histórico. Se surgir uma função operacional independente para Overview, ela deve ser demonstrada antes de manter uma quarta superfície. Esta é uma decisão de produto proposta, não uma mudança já aprovada.

| Área | Primeira visão | Aprofundamento |
|---|---|---|
| Execuções | Atividade/atenção recente e histórico com origem, modelo, data, estado e disponibilidade | Execução → métricas agrupadas → mesmas métricas por teste → tentativa → evidência/transcrito |
| Planos | Configurações salvas, última execução e estado | Escopo → execuções → baseline/candidato → comparação agrupada e por teste |
| Testes | Catálogo, disponibilidade e contagens com universo explícito | Contrato → histórico → comparação do teste |

Comparação é uma experiência compartilhada aberta a partir dessas áreas. A tela declara se compara execuções, planos ou versões de sistema, qual é a unidade pareada e quais diferenças impedem deltas. Isso não exige criar um novo objeto persistido “comparação”.

Os templates e as referências RC continuam sendo origens de configuração/evidência. Não precisam criar ciclos de vida de plano paralelos. Resultados remotos mantêm sua origem; execução local de uma referência continua sendo um novo experimento, com diferenças de ambiente explícitas.

Em cada execução, distinguir:

- **Ciclo operacional:** aguardando, executando, cancelando, encerrada ou requer reconciliação.
- **Validade técnica:** avaliação observável ou impedida por falha de infraestrutura/coleta/execução.
- **Conclusão da tarefa e nota:** conclusão observada e score disponível, sem converter falha técnica em zero.
- **Disponibilidade da evidência:** quais reports, tentativas e artefatos podem ser consultados.

Usar os campos existentes para essas dimensões; não adicionar outro enum de “resultado geral” que misture tudo novamente. Uma execução tecnicamente inválida pode conservar tempo ou consumo observado sem oferecer uma nota válida.

## 8. Nova organização de código

Organizar por responsabilidade em módulos do mesmo worker. As mudanças de pasta abaixo só devem ocorrer quando acompanharem a remoção de acoplamento; não há proposta de reescrever os módulos de cenários.

```text
src/
  worker.rs / manifest.rs        composição e registro
  control.rs                    admissão, execução e cancelamento
  plans/                        PlanStore e coordenação hoje dentro de dashboard/
  suite.rs / workflow/          execução dos cenários e fluxos
  scenarios/ / markdown/        definição, materialização e avaliação
  report.rs / result_contract.rs contrato nativo dos resultados
  journal.rs / durable.rs        transições e evidência arquivada
  persistence.rs                estado operacional e projeções SQL
  dashboard/                    APIs e projeções de leitura para o Console
  console_ui.rs                 publicação dos assets da interface

dashboard/src/
  console-entry.tsx / App.tsx    integração e rotas
  pages/                        composição das áreas de produto
  components/                   componentes E2E reutilizados por várias telas
  lib/                          acesso a dados e projeções puras existentes
```

A extração justificada é a coordenação de planos para fora de `dashboard/`, pois ela pertence ao worker mesmo sem uma tela. A UI continua usando o endpoint atual. Não criar um repositório genérico, service layer genérica ou pacote novo apenas para efetuar essa movimentação.

Na interface, a prioridade é reduzir os caminhos de apresentação concorrentes: expandir/reaproveitar PrimaryMetricsView e suas projeções onde o contrato permitir, manter diagnósticos com nomes próprios e retirar cálculos equivalentes espalhados nas páginas. Dividir uma página só quando houver responsabilidade independente ou componente reaproveitado; não fragmentar JSX por tamanho.

O cliente iii do host é o transporte do Console. Caminhos HTTP/static exclusivos do standalone podem ser retirados da aplicação à medida que seus consumidores forem identificados. A exportação de resultados, CLI, contratos usados pelo RC e execução protegida de CI têm função própria e continuam existindo.

As relações de domínio devem permanecer explícitas, sem usar “execução” para substituir todos os níveis:

| Entidade | Significado e vínculo |
|---|---|
| Plano | Configuração salva e materializada; possui histórico de execuções |
| Execução composta | Uma execução do plano; referencia seus grupos/filhos nativos |
| Execução nativa | Unidade admitida pelo ControlPlane; produz o bundle de resultados |
| Teste/cenário e versão | Contrato avaliado; não muda de identidade quando muda o modelo sujeito |
| Run e tentativa | Observação de um caso/repetição e suas tentativas técnicas, preservadas separadamente |
| Sessão | Conversa executada pelo Harness, vinculada ao run/tentativa/fase; pode ter descendentes |
| Evidência | Report, transcrito ou artefato referenciado pela observação; sua ausência não apaga a entidade operacional |

Essas relações orientam os módulos e as telas; não exigem um novo conjunto paralelo de DTOs ou tabelas.

## 9. Sequência de mudanças

| Etapa | Entrega concreta | Condição de conclusão |
|---|---|---|
| 0 — Diagnóstico, concluído nesta tarefa | Análise do código, auditoria das telas, documento e capturas | Fatos, WIP, propostas e limites identificados |
| 1 — Estabilizar dados, implementada localmente | Corrigir C2/C3; alinhar contrato de persistência C4; corrigir disponibilidade, contagens e diagnóstico incorreto | Lista/detalhe/histórico/plano concordam em fixtures e integração real |
| 2 — Isolar chats, implementada localmente | Contrato de escopo, produtores, herança, consultas/eventos, inspeção contextual e classificação histórica | Validado localmente; ativação depende de migração e implantação coordenadas |
| 3 — Consolidar contratos e responsabilidades, domínio de planos separado | PlanStore fora da apresentação; contrato único de sessão e armazenamento; projeções existentes reaproveitadas | Separação estrutural concluída; uniformização visual de métricas/comparações segue com o fluxo de referência |
| 4 — Redesenhar o fluxo de referência, implementada localmente | Execução de plano e comparação, com agrupado/por teste; evidência secundária | Validação com dados completos, parciais e inválidos no painel real do Console |
| 5 — Propagar e retirar duplicações, implementada localmente | Execuções, Planos, Testes; componentes do Console; retirada de caminhos standalone sem consumidor | Navegação consistente, menos CSS/adaptação, consumidores CLI/CI atualizados para o contrato único |

Etapas 1 e 2 podem avançar em paralelo após definir ownership dos arquivos. A etapa visual depende da semântica de resultados e do destino dos chats; não precisa esperar uma migração completa de armazenamento que não seja necessária ao fluxo.

Cada lote de implementação deve ser pequeno e revisável, com evidência e critério de aceite próprio. Corrigir o WIP existente antes de reorganizar suas pastas reduz o risco de esconder regressões em uma movimentação grande.

## 10. Aceite e validação da reestruturação

### Dados e execução

- Execuções nativas e compostas aparecem na lista e abrem por ID; filhos não duplicam totais.
- Cold start conserva lista, histórico, coortes e detalhe de execuções terminais com evidência preservada.
- Falta/expiração de bundle é distinguida de execução inexistente e de ausência total de telemetria.
- Falhas entre journal, persistência e finalização não provocam nova execução silenciosa do modelo; cancelamento e reconciliação mantêm identidade.
- Overview/lista paginada não exige carregar todos os transcritos.

### Chats

- Mais de 200 sessões E2E recentes não ocupam as vagas das conversas pessoais retornadas.
- Raízes, filhos, planners, workflows e retries não aparecem na sidebar/busca durante criação, atualização, reconexão ou restart.
- Classificação ocorre antes do primeiro evento; inspeção por ID não reintroduz a sessão no diretório pessoal.
- Reuso de ID humano, não classificado ou de outra execução/tentativa é rejeitado antes do turno; reuso idempotente da identidade E2E correta continua permitido.
- Transcritos/tentativas continuam acessíveis; snapshot ausente ou sessão viva indisponível é informado.
- Inspeção não muda a seleção global nem permite enviar à sessão avaliada. Se houver análise assistida, ela recebe outro ID.
- Migração preserva metadata e usa prova de origem; históricos sem prova e conversas pessoais são preservados.
- Ferramentas de métricas, árvore, exportação e cancelamento continuam vendo as sessões necessárias.

### Métricas e interface

- Input/output/cache, turnos, calls/errors, tempo, nota e gasto usam nomenclatura, unidades e populações consistentes.
- Score principal segue média por teste e entre testes; zero, ausente e parcial permanecem distintos.
- Filtro simétrico explica a granularidade, a cobertura removida e recalcula todas as vistas. O recorte filtrado não representa a qualidade global original.
- Deltas não atribuem ganho de eficiência a menor consumo causado por trabalho incompleto ou contrato incompatível.
- Back, links diretos, foco do modal, Escape e navegação por teclado preservam o contexto de execução/tentativa.
- Temas claro/escuro e painel estreito validam estados vazio, carregando, ativo, completo, falho, inválido, parcial e referência indisponível.

### Verificação realizada na etapa 0

- `pnpm typecheck`: passou.
- Vitest em `primary-metrics`, `plan-comparison`, `dashboard-data-source` e `scenario-chat`: 4 arquivos, 38 testes passaram.
- Navegação de leitura e capturas no Console, com os limites declarados na seção 5.
- Suíte Rust/Python completa, integração de persistência, CI remoto, release e deployment não foram executados nesta tarefa.

## 11. Decisões para discussão

A direção adotada é o isolamento lógico de sessões, a inspeção dentro do E2E e a correção das projeções antes do redesenho. O isolamento foi implementado conforme a seção 13. A seção 14 registra o fluxo visual de referência; a seção 15 registra sua propagação para as três áreas. A seção 12 registra a estabilização anterior dos dados.

Permanecem para a implementação: confirmar a revisão do runtime observado, fechar a granularidade de pareamento nas comparações com repetições e validar uma referência RC conectada. A análise assistida em novo chat é opcional e pode ficar para uma etapa posterior ao isolamento.


## 12. Implementação da etapa 1

Esta seção registra a etapa anterior. Seu backfill automático foi substituído pela migração explícita da seção 13; as tabelas SQL obsoletas vazias agora são removidas nessa migração, e a assertiva de teste desatualizada foi corrigida.

A implementação foi preparada em `feat/e2e-data-stabilization`, worktree isolado
`/home/layon/workspaces/harness-e2e-stabilize-data`, a partir do HEAD e do WIP
registrados na seção 2. A integração transfere somente o delta desta etapa,
após conferir os hashes dos arquivos originais.

### Mudanças entregues

- **C2:** o registro operacional SQL retém uma projeção compacta de resumo,
  identidade, coortes, versões e observações. Ela reutiliza o cálculo existente
  e preserva métricas ausentes como ausentes. Reports completos, prompts e
  transcritos continuam no bundle nativo. Não foi criada uma nova tabela.
- Execuções terminais legadas sem projeção são preenchidas na inicialização do
  dashboard a partir de evidência retida. Bundle ausente/inválido gera aviso por
  execução; não produz observações inventadas. Erro ao calcular a projeção não
  impede o commit operacional. Erros de banco continuam explícitos.
- A lista verifica a presença de resultado e manifest sem ler transcritos.
  A integridade do conteúdo é validada sob demanda no detalhe. Falha nessa
  leitura atualiza a disponibilidade do resumo em cache e retorna
  `evidence_error`, preservando identidade, status, métricas e histórico.
  Ausência de caminho nativo também é tratada como evidência indisponível.
- **C3:** pais de planos voltam à listagem com o vínculo dos filhos, e o detalhe
  do pai abre por ID. Totais aditivos do plano usam as projeções nativas já
  carregadas, sem reler os bundles dos filhos e sem somar apenas um subconjunto
  disponível como se fosse o total. Receipts corrompidos ou com plano ausente são isolados na
  consulta do dashboard e registrados no log. Operações de escrita e recovery
  continuam estritas.
- **C4:** foram removidas a criação das tabelas de planos sem consumidores e as
  APIs SQL genéricas sem uso. `PlanStore` continua responsável pelos arquivos
  JSON de definições/receipts; README agora declara essa fronteira. A alteração
  não apaga tabelas ou registros de instalações existentes.
- O catálogo separa total informado, linhas carregadas, testes disponíveis na
  visão e definições locais. Filtros e estados vazios identificam o universo
  carregado. Falhas de infraestrutura mostram a causa registrada no domínio
  correto; assets não avaliados não são chamados de inválidos.
- A execução mostra **Evidence bundle unavailable** e a causa de leitura, sem
  confundir um registro conhecido com uma execução inexistente.

### Validação local

O ambiente temporário usa um Engine separado em `127.0.0.1:49391`, o worker
`database` real e SQLite fora do banco do usuário. Foram copiados dois registros
terminais, um bundle nativo e um plano com seu receipt; nenhum modelo foi
executado. Um dos registros legados ficou sem bundle desde a primeira
inicialização, para verificar que isso não bloqueia o preenchimento do outro.

- O registro `d9c9ba46c1ce583c405d59824500185a` passou a ter projeção SQL, sem
  report completo dentro de `record_json`; sua falha real de infraestrutura
  continua sendo uma falha, sem conversão em sucesso.
- O plano `plan-6fcc6fe61fd3c381702643a9edd3d676` abriu com 11 slots e o filho
  nativo recebeu o vínculo do pai.
- Após retirar o bundle na cópia temporária e recarregar o processo, lista e
  detalhe indicaram evidência indisponível; status, totais, observação histórica,
  coorte e versão permaneceram iguais. O caso sem `result_path` também preservou
  o snapshot. Restaurar o bundle recuperou a disponibilidade da lista sem outro
  restart. ID inexistente continuou retornando erro.
- Adicionar um receipt corrompido após a inicialização não impediu a leitura do
  detalhe nativo válido.
- Vitest: **62 testes em 6 arquivos passaram**. TypeScript e builds standalone e
  Console passaram. Os builds mantêm o aviso de tamanho de chunk do Vite.
- Rust: **2 testes de persistência, 29 de controle e 58 de dashboard passaram**.
  A suíte ampla executada antes dos dois últimos testes adicionados terminou com
  **687 aprovados, 1 ignorado e 1 falha**. A falha é a assertiva preexistente de
  `native_coordination_covers_all_slots_including_capability_and_evolution` que
  espera rejeitar edição de plano travado, enquanto o contrato atual e seu teste
  `locked_plans_accept_scope_changes` permitem essa edição. Essa regra de edição
  não foi alterada nesta etapa; a suíte ampla não está declarada verde.

Os artefatos locais ficam em
`/tmp/harness-e2e-stage1-integration-sfea483r`. O [resumo de validação](design/restructure-stage1-2026-09-11/validation.json),
a [tela sem bundle](design/restructure-stage1-2026-09-11/evidence-unavailable.png)
e o [catálogo com contagens](design/restructure-stage1-2026-09-11/tests-counts.png)
foram preservados no repositório.
A validação no navegador utiliza a aplicação construída nesta etapa e conectada
ao ambiente temporário, sem erros de JavaScript. A carga de definições locais
permaneceu explicitamente indisponível porque essa stack não inclui `harness::send`;
a contagem do catálogo SQL e os estados de snapshot foram verificados. O Console do usuário em `:3113` não foi reiniciado.
Não houve commit, PR, CI remoto, release ou deployment. O isolamento dos chats
continua sendo a **etapa 2**.


## 13. Hard refactor: contrato e transição

A decisão de 11/09 substitui as hipóteses de compatibilidade da etapa 1. O plano é aplicado ao código existente, removendo os caminhos substituídos, sem criar um segundo serviço de sessão ou outro motor de execução.

- `src/plans.rs` e `src/plans/store.rs` passam a possuir definição, persistência, admissão e coordenação de planos. `src/dashboard/plan_projection.rs` mantém a projeção para as telas; o domínio não depende do dashboard em produção.
- O Session Manager passa a exigir `scope` em criação, consultas e assinaturas. A mesma seleção deve valer antes da paginação e na emissão de eventos. Filhos e forks herdam a origem, e colisões de sessão/correlação falham antes de anexar mensagens.
- O E2E abre o transcrito retido dentro da avaliação. Não seleciona a sessão avaliada no chat pessoal.
- Falha no transporte iii não dispara uma segunda chamada via HTTP. Os modos estáticos usados por publicação/auditoria continuam explícitos; a retirada do produto standalone e a reorganização visual fazem parte das etapas 4 e 5.
- O banco do E2E usa schema 2 e deixa de preencher projeções antigas no startup. `migrate-storage` faz dry-run por padrão e aplica todas as atualizações de registros e versão em uma transação. Execuções ativas impedem a migração; evidência ausente é declarada sem inventar score.
- A migração remove as tabelas SQL obsoletas `plans`, `plan_executions` e `plan_slots` somente quando estão vazias. Dados nessas tabelas bloqueiam o corte para inspeção; os arquivos autoritativos do PlanStore permanecem preservados.
- O runner interno de CI em `workers/harness/tests/e2e` conserva execução, catálogo e reports. Seu dashboard duplicado, sem assets existentes nem consumidor executável identificado, foi removido com as dependências exclusivas.

A implantação precisa coordenar Workers, Harness E2E e Console: encerrar execuções ativas; parar produtores; fazer backup; simular e aplicar as migrações; iniciar todos os consumidores com o contrato novo; renovar o Console e verificar descoberta/eventos/filhos. Reverter exige restaurar o conjunto de binários e o backup correspondente, sem misturar readers antigos e novos. Nenhuma migração em dados pessoais ou implantação é inferida de testes em fixtures.

### Isolamento entregue

O contrato exige `conversation` ou `e2e` na criação e `conversation`, `e2e` ou `all` nas consultas e assinaturas. As seis famílias de eventos incluem escopo, inclusive Bridge, exclusão e ressincronização. O Console consulta e assina conversas pessoais; inspeção de filhos e exportação pedem o universo necessário explicitamente. O escopo organiza a descoberta e não substitui autorização nas APIs de transcrito e armazenamento.

Sessões E2E novas exigem correlação `e2e_*_id` não vazia. O Harness valida identidade antes de send, inject e reuse; filhos e forks herdam escopo e correlação. A comparação de identidade também ocorre sob o mutex do armazenamento autoritativo, protegendo chamadas simultâneas de serviços Bridge distintos. Produtores de suíte, Markdown, planner, workflow, SWE e o runner interno de CI usam o contrato novo; os consumidores Workers foram atualizados juntos.

O migrador `workers/session-manager/scripts/migrate_session_scopes.py` classifica o histórico completo de metadata e a árvore de pais/forks. IDs genéricos e títulos não servem como prova E2E. IDs exatos fornecidos e ausentes do armazenamento, pais sem origem resolvida ou escopos conflitantes impedem aplicar. A execução com `--apply --backup-dir` preserva mensagens, metadata, dono/grupo, modo e timestamps; prepara backups e substitui cada arquivo atomicamente, com rollback de erros durante a aplicação. A operação pressupõe todos os escritores parados. Históricos sem prova E2E tornam-se conversas pessoais; não há classificação implícita no reader novo.

### Validação do hard refactor

O [registro de validação](design/restructure-hard-2026-09-11/validation.json) identifica comandos, resultados e limites. As contagens abaixo pertencem a suítes diferentes; execuções focadas que se sobrepõem não foram somadas.

- Harness E2E: **691 testes de biblioteca aprovados, 1 ignorado**, mais 12 de CLI e 1 de manifesto. A assertiva antiga de edição de plano foi alinhada ao contrato já vigente, preservando o receipt da execução materializada.
- Session Manager: **83 unitários, 4 schemas e 1 manifesto**. Em Engine temporário real, **159 cenários BDD e 1.479 passos passaram**, sem pular os cenários que dependem do Engine.
- Harness: **488 testes de biblioteca**, 3 de CLI, 5 de manifesto, 5 de prompts, 3 de schemas e 120 no pacote de integração. O runner interno de CI passou nos **55 testes** e no Clippy com `-D warnings`.
- Dashboard: **277 testes em 45 arquivos**, typecheck e builds standalone/Console. Console: conjuntos focados de 85 e 106 testes, typecheck e build. Mantém-se somente o aviso de tamanho de chunk do Vite.
- Consumidores: testes de Workflow, Slack, Telegram, OpenWiki e UIs afetadas passaram; Claude Code **14/14** e Pi **15/15**, com typecheck e lockfiles preservados. Memory, Approval Gate, Eval e Security Scan compilaram nos checks executados.
- Migração de sessões: **5 testes** cobrem evidência histórica, herança, conflitos, IDs fornecidos desconhecidos, backup, rollback, preservação dos arquivos e repetição sem alteração.
- A [migração SQL real](design/restructure-hard-2026-09-11/storage-migration.json) validou dry-run sem alteração, rejeição de execução ativa e de tabela obsoleta populada, remoção das tabelas vazias, projeções e checksums na versão 2, ausência de report duplicado, repetição sem alteração e leitura após restart. O reader novo recusou o banco antigo com instrução explícita de migração. Foram usados dois registros terminais copiados e evidência retida em diretório temporário; nenhum modelo foi executado.
- Testes automatizados cobrem mais de 200 sessões E2E recentes, paginação pessoal, eventos e concorrência de identidade entre serviços. No navegador, uma fixture de interação abriu transcritos retidos da tentativa atual e de retry, sem selecionar o chat do host e sem erros de página. Essa checagem não equivale à aceitação visual do Console completo.

A revisão independente não encontrou P1/P2 pendente no corte examinado. A integração nos checkouts principais transfere apenas o delta contra a cópia inicial e verifica hashes, preservando o WIP anterior. Não houve commit, PR, CI remoto, migração dos dados do usuário ou deployment; o Console em `:3113` não foi reiniciado. A ativação do isolamento depende do corte coordenado descrito acima. O fluxo de referência foi implementado na etapa 4 abaixo. A propagação e a retirada dos caminhos standalone sem consumidor foram implementadas na etapa 5, registrada na seção 15.


## 14. Etapa 4: fluxo visual de referência

O plano local e o detalhe de sua execução usam o mesmo `PrimaryMetricsView`.
**Grouped** apresenta nota, gasto, tempo, tokens e atividade; **By test** apresenta
os mesmos resultados por teste e versão, nas abas Overview, Tokens e Activity.
Somente a vista escolhida ocupa a tela. O componente público de abas do Console,
os tokens do host e as regras de largura do painel foram reaproveitados.

A comparação identifica A como referência visual e B como comparação. Os
seletores mostram nome, modelo, data e estado da execução; A pode ser alterado
sem modificar a baseline oficial do plano. Links abrem cada execução e o
breadcrumb da execução retorna ao plano. **Execution only** permanece uma
seleção explícita durante as atualizações da execução ativa.

O cálculo existente em `primary-metrics.ts` permanece a fonte das duas vistas.
O filtro retira os mesmos testes de A e B e recalcula médias, consumo e deltas.
Zero, resultado ausente e observação parcial continuam distintos. Foram retirados
os gráficos dumbbell duplicados do plano, a projeção usada apenas pelo cartão de
custo repetido e as linhas de custo, duração e chamadas já cobertas pelas métricas
principais. Estatísticas de runs, cobertura, retries e histórico continuam nos
painéis de diagnóstico, pois respondem a perguntas diferentes da média por teste.

Cada teste oferece **Transcript A/B**, limitado à versão selecionada. Tentativa
atual e retries abrem o transcrito retido no E2E. O menu aceita teclado; Escape
retorna o foco ao botão e conserva vista/filtro da comparação. Consultas por ID
são refeitas a cada abertura: o cache permanente e o bloqueio definitivo após
uma consulta vazia foram removidos, permitindo descobrir evidência que chegou
durante uma execução. A busca no transcrito também recebeu espaçamento correto
para seu ícone. Não há seleção nem envio de mensagem ao chat pessoal nesse fluxo.

### Validação e limites

O [registro de validação](design/restructure-stage4-2026-09-11/validation.json)
identifica os estados e as capturas. O navegador carregou o SPA real do Console,
seus componentes compartilhados e o bundle desta etapa. RPCs foram atendidos por
fixtures em um servidor temporário: os valores ilustram estados da interface e
não constituem uma nova execução de benchmark ou medição de modelos.

- Vitest: **277 testes em 45 arquivos passaram**. TypeScript e builds standalone/Console passaram; permanece o aviso de tamanho de chunk do Vite.
- No navegador: filtro simétrico 4 → 2 → 4, médias 86/93 e delta +7 no recorte ilustrativo; seleção Execution only após atualização ativa; teclado, foco, busca e transcritos de A/B e retries; retorno execução → plano; estados vazio, parcial, sem bundle e tecnicamente inválido. Nenhum erro de página nas verificações concluídas.
- Capturas em temas claro/escuro e larguras de viewport 1440/720/600/390, com métricas em painéis de 1320 até 320 px. A revisão independente não deixou P1/P2 ou defeito visual bloqueante no recorte examinado.

Veja o [plano e a comparação](design/restructure-stage4-2026-09-11/plan-comparison-top.png),
a [vista por teste](design/restructure-stage4-2026-09-11/comparison-by-test-dark.png)
e o [transcrito contextual](design/restructure-stage4-2026-09-11/transcript-current.png).

O Console do usuário em `:3113` serviu o SPA, mas seu WebSocket desconectou sem
responder às consultas. A validação interativa usou um transporte temporário
isolado, sem reiniciar esse Console, executar modelos ou alterar dados pessoais.
A aceitação com o runtime do usuário conectado permanece pendente. O isolamento
real das sessões ainda depende do corte coordenado da seção 13.

### Próxima etapa

Propagar o padrão validado para as áreas de Execuções, Planos e Testes, incluindo
a comparação com referência RC; retirar a navegação/chrome standalone e as
adaptações sem consumidor; consolidar os diagnósticos restantes sem confundir
medianas de runs com a média principal por teste. A seleção/filtro acompanha a
inspeção contextual, mas ainda não é serializada em um link compartilhável.


## 15. Etapa 5: Console como interface única

A navegação principal reúne **Execuções**, **Planos** e **Testes**. O Overview
foi retirado; Execuções é a entrada padrão e reúne histórico, criação de plano
e execução rápida. Planos distingue configurações locais, referências do
Release Control e templates pelas abas do Console. O catálogo explicita as
contagens carregadas e a última execução. O histórico prioriza os resultados;
contrato e tendência ficam disponíveis como investigação. Suas medianas são
identificadas como medianas dos resumos de execução, sem equivalência implícita
com a média principal por teste ou uma mediana de todos os runs.

A comparação RC reutiliza `PrimaryMetricsView`, com vistas Grouped/By test e
filtro simétrico. Dados RC são projetados diretamente do contrato de referência,
sem fabricar reports nativos. A nota agrupada tem peso igual por teste. Deltas
exigem a mesma versão, caso, seed, repetição, definição, contratos de
resultado/avaliação, modelo/juiz e política de retries, além de execuções
terminadas e reprodução sem diferenças. Escopo ou distribuição incompletos
não recebem métricas completas. Total
de tokens pode ser comparado; input/output/cache, function errors e gasto total
RC permanecem indisponíveis quando o ledger não fornece grandezas equivalentes.

O frontend recebe cliente iii, tema, fontes e componentes do Console. Foram
retirados bootstrap e servidor standalone, proxy HTTP/WebSocket, transportes
HTTP/estático, aliases de navegação, seletor próprio de tema, fontes embarcadas
e reescrita de paleta no build. O build produz apenas `dist-console`. Regras
sem consumidor saíram do CSS legado, que conserva somente os estilos ainda
usados. Os seletores do bundle continuam limitados à extensão.

CLI de execução, funções iii, armazenamento/leitura de resultados e publicação
JSON continuam como consumidores ativos. O comando `dashboard`/`serve` foi
retirado da CLI. Os testes de navegador usam um host funcional explícito do
contrato Console; a validação visual usa o SPA real do Console em um preview
com RPCs controlados. Nenhum deles representa uma execução de benchmark.

### Validação e limites

Registro final e capturas: [validação da etapa 5](design/restructure-stage5-2026-09-11/validation.json).

- Rust: **709 testes passaram, 1 ignorado**; formatação e Clippy com warnings tratados como erro passaram.
- Frontend: **261 testes em 44 arquivos**, TypeScript, build Console e lint passaram. Permanece um aviso de assertion não nula em fixture de teste anterior. Os 26 testes de navegação/plano também passaram após corrigir os links de criação.
- Publicação JSON: **7 testes Python**. Contratos de estilo e ratchet CSS: **9 testes**.
- Navegador: planos, referência RC, navegação entre três áreas, formulários, filtro simétrico, métricas parciais, transcritos/retries e teclado. Temas claro/escuro e viewport até 390 px, sem overflow global nas amostras medidas.
- Paleta e fontes herdadas do Console verificadas por estilos computados. Contraste dos textos/estados amostrados em três superfícies: mínimo **4,59:1 no claro** e **5,64:1 no escuro**, considerando transparência.
- Revisão independente sem observação acionável restante no delta. As correções de pareamento, escopo e política de retries RC têm regressões unitárias específicas.

Veja [Execuções](design/restructure-stage5-2026-09-11/executions-desktop.png),
[Planos](design/restructure-stage5-2026-09-11/plans-desktop.png) e
[Testes em painel estreito](design/restructure-stage5-2026-09-11/tests-390.png).

A aceitação com o runtime do usuário conectado, a migração histórica e a
implantação coordenada continuam pendentes. Esta etapa não reiniciou o Console
em `:3113`, não executou modelos e não alterou as sessões pessoais. A ativação
do isolamento descrito na seção 13 depende desse corte coordenado.
