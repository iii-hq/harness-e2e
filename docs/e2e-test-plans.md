# Plano mestre de testes e medição do Harness

Este é o ponto único de definição da estratégia de testes do Harness. O plano
reúne cobertura, propósitos, métricas, amostragem e evolução do instrumento.
Os perfis selecionam trabalho desse mesmo plano; a cadência define quando ele
roda. Cada execução preserva sua configuração e seus resultados imutáveis.

**Estado em 2026-09-05:** os seis perfis estão implementados sobre Results v4.
[`config/test-plan.json`](../config/test-plan.json) é a fonte executável única;
CLI, dashboard, campanhas e [composição gerada](test-profiles.generated.md)
consomem essa definição. Os manifests anteriores são saídas de compatibilidade,
com a mesma semântica. Esta implementação foi validada sem executar modelos;
qualificação dos perfis, calibração e migração da agenda de produção permanecem
nas etapas F3–F5.

## 1. O que está sendo unificado

Há seis manifests versionados, com 29 cenários distintos e três configurações de
fault injection. As contagens são slots solicitados antes de retries;
as fases de fault têm seu próprio orçamento de soak.

| Manifest atual | Grupos | Slots | Destino no plano mestre |
| --- | ---: | ---: | --- |
| [`daily`](../config/campaigns/daily.json) | 4 | 11 | Regressão; cadência diária |
| [`post-deploy`](../config/campaigns/post-deploy.json) | 5 | 9 | Smoke da versão publicada e integração selecionada |
| [`weekly`](../config/campaigns/weekly.json) | 14 | 48 | Capacidade e resiliência, com orçamentos próprios |
| [`endurance`](../config/campaigns/endurance.json) | 1 | 1 | Endurance |
| [`swe-isolated`](../config/campaigns/swe-isolated.json) | 8 | 8 | Engenharia em regressão, capacidade e evolução |
| [`swe-continuous`](../config/campaigns/swe-continuous.json) | 1 | 1 | Jornada de engenharia em endurance |

O rascunho local de evolução adicionava 20 cenários à união dessas campanhas:
a matriz abaixo cobre os 49. Outros cinco cenários do catálogo ficam explícitos
como diagnósticos. As sete tarefas nativas do worktree de benchmarks são uma
extensão em avaliação, com contratos próprios, ainda ausente deste checkout.

As descrições antigas divergiam dos manifests: o diário já contém cinco casos
Markdown, e `minimal_path` não está no `post-deploy` atual. O semanal contém
fault injection, enquanto endurance tem manifest separado. O resumo em inglês
e o documento de evolução passam a apontar para este plano. O rascunho de
evolução foi [preservado como histórico](version-evolution-2026-09-05.archive.md).

## 2. Um plano, seis perfis

| Perfil | Pergunta respondida | Seleção inicial | Amostragem inicial | Uso do resultado |
| --- | --- | --- | --- | --- |
| `smoke` | A stack publicada executa os caminhos essenciais? | 5 casos abaixo | 1 por caso; 0 retries | Disponibilidade e correção observada nessa execução |
| `regression` | Uma mudança quebrou comportamentos representativos? | 12 casos abaixo | 1 por caso; até 1 retry técnico onde houver replay seguro | Localizar regressões e abrir investigação |
| `capability` | Quais capacidades e níveis de dificuldade funcionam? | M1–M8; 47 casos em grupos isolados | Piloto com 1; depois 3 por caso qualificado | Cobertura por domínio, repetibilidade e limites |
| `evolution` | O Harness mudou em qualidade, confiabilidade ou eficiência? | Painel fixo de 18 casos abaixo | Piloto com 5 por caso e por versão | Comparação A/B inicialmente descritiva |
| `resilience` | O trabalho se recupera de falhas sem efeitos duplicados? | Matriz L2–L4 e casos de recuperação abaixo | 3 por fase de fault; preservar soak de 60 min | Correção pós-falha, cleanup e amplificação |
| `endurance` | Até onde uma execução contínua sustenta trabalho correto? | 5 casos abaixo, com execuções independentes | 1 por caso; 0 retries | Maior prefixo aceito e motivo da parada |

Essas seleções são hipóteses de composição a qualificar; ainda não há evidência
comparativa suficiente para chamá-las de ótima relação cobertura/custo.
Todos os perfis permanecem advisory para deployment e promoção. Falhas objetivas
continuam visíveis como falhas, mesmo sem bloquear uma release.

Release Control continua responsável por agenda, admissão e dispatch de
`.github/workflows/exact-stack-e2e.yml`. `harness-e2e` materializa, executa,
avalia e arquiva a evidência nativa. `daily`, `weekly` e `post-deploy` tornam-se
seleções de perfil/cadência na migração, preservando seus IDs históricos.

### Seleções implementadas para qualificação

**Smoke — 5 casos:** `minimal_path`, `tool_contract_recovery`, `persistent_state`,
`timer_wake`, `shell_coder_sandbox`. Medir o custo do canário de código antes de
definir um SLA de duração. Browser e serviços adicionais entram como extensão
explícita quando fizerem parte da stack publicada.

**Regressão — 12 casos:** os cinco de smoke, mais `performance_regression`,
`git_regression_forensics`, `database_migration_recovery`, `contention_ledger`,
`swe_config_isolation`, `swe_cache_invalidation` e `swe_replay_recovery`.
SWE exige ambiente isolado próprio; ausência desse ambiente conta como cobertura
não executada. Alterações em Git handoff, browser, segurança ou adaptação
acrescentam os respectivos casos da matriz por seleção explícita.

**Evolução — painel fixo de 18 casos:** `minimal_path`, `tool_contract_recovery`,
`persistent_state`, `timer_wake`, `context_pressure`, `validation_self_repair`,
`contention_ledger`, `quorum_fan_in`, `shell_coder_sandbox`,
`performance_regression`, `swe_config_isolation`, `swe_cache_invalidation`,
`swe_replay_recovery`, `swe_contract_migration`, `engineering_ticket_git_handoff`,
`cross_app_transaction`, `prompt_injection_resilience` e `incident_response`.
São 90 slots por versão no piloto com cinco repetições, 180 no par A/B, antes
de retries. Browser, scanner de segurança, migração entre repositórios e
endurance mantêm relatórios próprios; esse painel não representa o catálogo todo.
Usar zero retries no piloto A/B para controlar o tratamento; qualquer política
posterior deve ser igual nos dois lados e integrar a identidade da coorte.

**Resiliência:** `weekly-l2-recovery` / `stateful.2`, `weekly-l3-recovery` /
`coordination.3` e `weekly-l4-recovery` / `coordination.4`, mais
`cleanup_under_failure`, `poison_message`, `subagent_validation_failure` e
`swe_replay_recovery`. Para esses quatro casos, iniciar com uma execução sem
retry e expandir para três após qualificação. Preservar tetos de amplificação
e demais critérios dos [perfis de fault](../config/profiles/).
Falhas injetadas são parte do tratamento, não motivo para retry técnico.

**Endurance:** `engineering_endurance_ladder`, `swe_service_journey`,
`wake_chain_soak`, `fanout_ladder` e `depth_ladder`. Os dois primeiros medem
engenharia cumulativa; os demais medem duração, largura ou profundidade de
coordenação. Cada um conserva sua unidade de capacidade e seu orçamento.
Capacidade usa zero retries no piloto; posteriormente, no máximo um retry
técnico nos casos comprovadamente replay-safe, com custo integral contabilizado.

## 3. Matriz única de cobertura

Cada cenário tem um módulo primário e pode servir a mais de um perfil. Reutilizar
um caso não cria outra definição nem autoriza somar a mesma observação duas vezes.
O módulo indica a capacidade medida; `execution_kind` determina como executar.

| Módulo | Cenários | Evidência e medida específicas |
| --- | --- | --- |
| M1 — Caminho básico e estado | `minimal_path`, `persistent_state`, `insert_record`, `sequential_pipeline`, `database_migration_recovery` | Estado final, ordem de etapas, backfill, quarentena, idempotência, preservação de sentinel e cleanup |
| M2 — Contexto e instruções | `context_pressure`, `moving_target`, `mechanical_reaction` | Continuidade com pressão de contexto, atendimento à mudança e execução da reação esperada |
| M3 — Wakes, validação e recuperação | `timer_wake`, `validation_loop`, `validation_self_repair`, `validation_scope_enforcement`, `validation_chain`, `cleanup_under_failure`, `poison_message`, `wake_chain_soak` | Wake registrado na ordem correta, retomada, reparo, limites de validação, efeitos únicos e recursos encerrados |
| M4 — Coordenação | `contention_ledger`, `quorum_fan_in`, `subagent_validation`, `subagent_validation_failure`, `fanout_ladder`, `depth_ladder`, `research_pipeline`, `receiving_operation` | Árvore completa, fan-in correto, ausência de lost update, propagação de falha, evidência dos filhos e amplificação |
| M5 — Engenharia | `shell_coder_sandbox`, `performance_regression`, `chess_engine_build`, `git_regression_forensics`, `engineering_ticket_git_handoff` | Reprodução vermelha, patch aceito por testes públicos/ocultos, resultado de CLI/oracle, custo algorítmico, commit culpado e integridade Git |
| M5 — Engenharia SWE isolada | `swe_config_isolation`, `swe_cache_invalidation`, `swe_batch_replay`, `swe_replay_recovery`, `swe_contract_migration`, `swe_tenant_isolation`, `swe_replay_performance`, `swe_release_handoff` | Comportamento do serviço, crash real pós-commit, cliente legado revelado, isolamento entre tenants, SQL limitado e checkpoint imutável |
| M6 — Integração | `tool_contract_recovery`, `cross_app_transaction`, `browser_cross_site` | Descoberta de contratos, rejeição do decoy, CAS, snapshots/auditoria e estado de backend após navegação entre origins |
| M7 — Segurança e política | `prompt_injection_resilience`, `secret_hygiene`, `security_review`, `policy_bound_action` | Ausência de efeitos proibidos ou vazamento, comportamento sob conteúdo adversarial, revisão/scanner e autorização no diálogo |
| M8 — Adaptação operacional | `incident_response`, `cross_repo_contract_migration`, `release_train_recovery` | Evidência que invalida o plano, replanejamento, preservação de trabalho aceito, compatibilidade e recuperação sem dupla remediação |
| M9 — Engenharia contínua | `engineering_endurance_ladder`, `swe_service_journey` | Maior prefixo de tickets aceitos, preservação das entregas anteriores, checkpoints rejeitados e motivo da parada |

Detalhes dos verificadores continuam nos contratos dos cenários. As regras de
[Markdown](markdown-scenarios.md), [SWE](swe-service.md),
[Git handoff](engineering-ticket-git-handoff.md), [L5](l5-adaptive-scenarios.md),
[fault injection](fault-injection.md) e [endurance](engineering-endurance-ladder.md)
são documentação técnica subordinada a esta estratégia, sem composição de planos
concorrente. Os pesos de `difficulty-weighted-v1` não mudam nesta reformulação.

### Casos diagnósticos fora dos perfis iniciais

| Cenário | Destino e critério de inclusão futura |
| --- | --- |
| `engineering_ticket` | Comparar cobertura com Git handoff e SWE antes de retirar ou promover |
| `trend_blog` | Diagnóstico de pesquisa, grounding e conteúdo; precisa de métrica própria de fontes e afirmações |
| `todo_worker_simple`, `todo_worker_planned` | Integração de lifecycle de workers em Registry/Compose isolados e descartáveis |
| `chess_play_ladder` | Capacidade interativa prolongada; qualificar dispersão e orçamento antes de incluir em endurance |

Caso bloqueado por fixture ou worker permanece na matriz com motivo, responsável
e requisito de retorno. Uma falha de setup não demonstra redundância nem autoriza
a retirada do denominador de cobertura planejada.

### Critério para substituir cenários por tarefas nativas

O worktree `harness-e2e-benchmarks` contém uma suíte de desenvolvimento com cinco
repetições de `bugfix_config_precedence`, `bugfix_cache_invalidation`,
`feature_batch_replay`, `security_code_review`, `contract_migration_plan`,
`release_train_recovery` e `release_train_recovery_simulated`. Seu manifest declara
`official_verifier_required: false`; essa execução não equivale automaticamente
a uma avaliação oficial com verifier privado. A integração deve fixar uma revisão
de runner disponível e mapear seus resultados ao contrato comum. Distinguir
`task:release_train_recovery` de `scenario:release_train_recovery` na identidade.

Há sobreposições úteis, mas o rascunho anterior superestimava equivalência:

| Substituição sugerida anteriormente | Decisão deste plano |
| --- | --- |
| `shell_coder_sandbox` por bugfix tasks | Medir paridade de reprodução vermelha, escopo, probes e CLI; comparar custo antes de reduzir frequência |
| Migração adaptativa por `contract_migration_plan` | Preservar ambos: a task escreve um artefato, protege o código e não executa a migração em três repositórios |
| `security_review` por `security_code_review` | Preservar scanner, deduplicação e reconciliação; o JSON de review mede outra fronteira |
| Release adaptativa por recovery tasks | Artefato, simulação stateful e interação adaptativa conservam execuções e rótulos distintos |
| Retirar `trend_blog` por cessão à task layer | Não há substituto demonstrado; manter lacuna de grounding explícita |

Só consolidar casos depois de uma matriz de requisitos mostrar que o substituto
preserva todos os sinais relevantes. Similaridade de nome ou taxa de aprovação
correlacionada não basta. Verificação determinística também existe nos cenários;
a distinção útil é a fronteira exercitada, o isolamento e a força do oracle.

## 4. Contrato de metrificação

O resultado do plano é um conjunto de indicadores por propósito e domínio.
Conclusão, correção, validade técnica e disponibilidade de qualidade seguem o
[contrato Results v4](result-reliability.md). Não reinterpretar a nota como
probabilidade de sucesso nem conclusão como entrega correta.

| Dimensão | Indicador principal | Denominador e leitura |
| --- | --- | --- |
| Execução confiável | `execution_reliability` | Runs tecnicamente válidos / slots planejados; mostrar também deferred e inválidos |
| Conclusão | `completion_rate` e `completion_evidence_coverage` | Completed / (completed + task_incomplete), acompanhado de cobertura sobre planejados |
| Correção | Outcome objetivo, gates e `deliverable_success` quando aplicável | Numerador/denominador de resultados avaliáveis, com `objective_score_coverage`; falhas e inconclusivos visíveis |
| Qualidade | `quality_score_completed` e `quality_coverage` | Mediana disponível entre completed; informar quais critérios são determinísticos ou julgados |
| Integridade | Dimensões estruturais, grounding, isolamento e cleanup | Taxas onde a dimensão existe; ausência é indisponibilidade, não aprovação |
| Eficiência | `total_tokens_consumed`, `tokens_per_completion`, `tokens_completed_p50` | Consumo cumulativo do subject, inclusive tentativas falhas; conclusão não implica correção |
| Diagnóstico de custo | `failed_attempt_tokens`, `judge_tokens_consumed`, turns e function calls | Judge separado do subject; raiz e filhos explicitados; custo de retry contado uma vez |
| Resiliência | Recuperação correta, efeitos duplicados, cleanup e amplificação | Por cenário/perfil de falha e orçamento; identificar a composição de qualquer agregado |
| Endurance | Maior prefixo aceito e motivo da parada | Por ladder/jornada; capacidade e limite de orçamento separados de falha técnica |

**Implementado:** Results v4 conserva os indicadores acima. O comparador
longitudinal e `test-plan measure` agora expõem `consumption`, com tokens totais,
tokens por conclusão e por sucesso verificado, p50/p95 de tokens e function calls,
contagens e motivos de indisponibilidade. O comparador publica também seus deltas,
sem adicionar gates de promoção. Os campos existentes de Results v4 mantêm sua
semântica; a nova projeção aplica verificação estrita de completude das tentativas.

`tokens_per_verified_success` divide todo consumo observado do subject, inclusive
falhas técnicas e retries, pelos runs completed, tecnicamente válidos, com status
objetivo Passed, score objetivo disponível, gates obrigatórios presentes e todos
aprovados, e entregáveis aprovados quando presentes. O denominador é publicado.
Usage incompleto ou retry sem telemetria torna o consumo indisponível; zero sucessos
produz `null`. A qualificação empírica dessa leitura continua pendente.

USD só entra como medida quando há informação de preço/consumo válida. Um zero
sem semântica de faturamento conhecida não comprova gratuidade. Dados ausentes e
denominador zero produzem `null` com motivo. Se algum attempt observado não tiver
usage, não estimar o custo da execução pela tentativa final. Cache, tokens de
raciocínio e tokens faturáveis precisam respeitar a semântica do provider para
evitar dupla contagem.

A apresentação mostra taxas por caso e módulo, amostra, cobertura e custo total.
Uma média entre módulos exige pesos fixados antes da execução e identidade
própria; não usar uma mistura variável de casos ou mais repetições de um cenário
para melhorar a nota global. Um mesmo run selecionado por dois perfis participa
uma única vez do total de consumo.

## 5. Evolução comparável e limites estatísticos

**Eixo declarado.** Para medir evolução do Harness, variar sua revisão/artefato
e fixar modelo/provider, configuração, engine, demais workers, fixture, verificador,
contratos de resultado/scoring, casos, budgets e política de retry. Se engine ou
outros componentes também variarem, rotular como evolução da stack.
Mudança de modelo forma outro experimento; o comparador atual não permite essa
troca como se fosse a mesma coorte. Mudança no próprio evaluator exige série
separada ou estudo de ligação entre versões, sem comparação direta automática.

**Identidade completa.** Persistir `plan_id`, revisão/digest do plano, perfil,
`treatment_axis`, identidade integral da stack e digest dos fatores fixos, além
de case/seed/input/contract digests e IDs de execução/tentativa. Seeds continuam
pertencendo aos cenários; campanhas atuais proíbem seleção ou rotação de seeds.
Nova coorte de casos requer materialização e identidade revisadas.

**Desenho A/B.** Intercalar e randomizar a ordem das versões dentro de blocos de
caso/modelo/ambiente, preservando a programação para replay. O pareamento por caso
controla diferenças de dificuldade; mesma seed de fixture não garante a mesma
trajetória de um provider não determinístico. Essa é uma aplicação do desenho
por blocos descrito pelo [NIST](https://www.itl.nist.gov/div898/handbook/pri/section3/pri332.htm).
Comparações históricas sem esse controle conservam os deltas como descritivos.

**Amostragem.** O código chama n < 5 de `directional`, n ≥ 5 de `repeatable` e
n ≥ 20 de `validated`; p95 exige pelo menos 20 observações comparáveis e completas
por caso/lado. São rótulos e limites de publicação do instrumento. Não demonstram
poder estatístico, precisão de cauda ou ausência de regressão.
Um piloto com cinco runs estima variabilidade; o tamanho confirmatório depende
do efeito mínimo relevante e da precisão desejada. Repetições do mesmo caso
medem consistência nesse caso, sem ampliar a diversidade de tarefas.

**Ordem de decisão.** Primeiro exigir cobertura, validade técnica, piso absoluto
de correção e integridade. Depois testar não inferioridade de correção com margem
predefinida. Só então interpretar eficiência condicionada a sucesso como possível
ganho. Ausência de diferença detectada, intervalos sobrepostos ou `separated: false`
não provam equivalência. Sem poder suficiente, publicar `inconclusive` ou
`descriptive_only` na proposta de interpretação, sem transformar isso em novo gate.

**Estatística a qualificar.** Escolher uma métrica primária por experimento e
pré-fixar estimador, efeito mínimo, intervalo, unidade amostral, tratamento de
zeros e múltiplas comparações. Para economia, começar pela medida de tokens
definida na seção anterior; tempo e chamadas servem ao diagnóstico. Não dividir
por baseline zero nem chamar um estimador de deslocamento de razão de medianas.
Se vários casos/métricas decidirem um resultado, controlar multiplicidade, por
exemplo com Holm; outras leituras ficam exploratórias. Reamostragem precisa
respeitar agrupamento por caso, par A/B e dependências de ambiente.

**Qualificação do instrumento.** Rodar A/A com replicações independentes e ordem
randomizada, além de A/B com degradações conhecidas. Medir falsos alarmes, poder,
estabilidade dos verificadores e cobertura; repetir o mesmo cálculo com a mesma
seed testa determinismo do cálculo. Aumento de n não corrige oracle defeituoso.
O [harness do SWE-bench](https://www.swebench.com/SWE-bench/api/harness/) é uma
referência de verificações de correção e preservação de comportamento por testes;
esta proposta adota esse princípio, sem assumir equivalência com seu benchmark.

**Histórico.** Arquivar evidências nativas com `e2e::archive` e retenção
`longitudinal`, caminho já usado pelo executor exato. Preservar observações por run,
tentativas, outcomes, tokens, turns, calls, disponibilidade e identidades; medianas
sozinhas não permitem recalcular comparações. Results v2/v3 permanecem visíveis
como históricos incompatíveis, sem backfill ou baseline comparável inventado.
Validar paginação/retenção de toda a série, além da primeira página do dashboard.

## 6. Materialização e orçamento

A definição versionada contém módulos, seletores, repetições, retries e métricas.
O materializador nativo resolve IDs, seeds, contratos e envelopes a partir do
catálogo do próprio runner. Cada rodada gera uma campanha com uma invocação por
cenário; casos adaptativos conservam `runs=1`. Os snapshots fixam digests de fonte,
perfil e campanha. O parser rejeita divergência em relação à composição gerada,
inclusive se o manifest alterado receber um novo hash.

O dashboard apresenta cobertura, orçamento e exportação de cada perfil. O runner
importa essa exportação e exige paridade com a definição do binário fixado.
Planos personalizados de comparação baseline/candidate continuam disponíveis.
Execuções usam diretório exclusivo, recibo de execução e bundles nativos; o
recibo retém progresso parcial se houver interrupção ou falha na coleta.

```bash
# Listar, verificar a geração e visualizar sem chamar modelos.
cargo run --locked -- test-plan list
cargo run --locked -- test-plan sync --check
python3 scripts/run_test_plan.py --profile smoke

# Exportar uma revisão imutável para inspeção (o destino deve ser novo).
python3 scripts/run_test_plan.py --profile evolution --export /tmp/harness-evolution-v1

# Executar na stack configurada, com identidades explícitas.
python3 scripts/run_test_plan.py --profile smoke --execute \
  --model "$HARNESS_E2E_MODEL" --provider "$HARNESS_E2E_PROVIDER"

# Usar o perfil exportado pelo dashboard; por padrão, apenas visualizar.
python3 scripts/run_test_plan.py --import-profile /path/to/harness-smoke.json

# Consolidar Results v4 de invocações independentes, sem misturar coortes.
cargo run --locked -- test-plan measure \
  --results /path/to/round-1/results.json \
  --results /path/to/round-2/results.json
```

O wrapper usa `target/debug/harness-e2e` por padrão; `--e2e-bin` fixa outro binário.
Perfis com Markdown exigem `--judge-model` e `--judge-provider` na execução.
`HARNESS_E2E_SEED` deve estar ausente: a seed canônica pertence ao cenário.
Para resiliência, usar `--export` com subject e judge explícitos; são geradas suites
compatíveis com o contrato de Release Control, com `seed: null` e a identidade do
plano preservada. Fault injection executa somente pelo supervisor protegido.
A exportação não agenda nem despacha uma execução de produção.

Após editar a fonte, rodar `cargo run --locked -- test-plan sync` e versionar as
saídas. A CI verifica os seis manifests de compatibilidade, os catálogos de
admissão/perfis e a documentação gerada. Não editar essas saídas manualmente.

Regras a preservar na geração:

1. Validar cada cenário, seu `execution_kind`, dificuldade, pré-requisitos e
   disponibilidade no runner fixado antes de admitir uma execução.
2. Separar grupos por replay safety, ambiente/fixture e orçamento. Diálogos,
   composites, adaptativos e fault injection usam zero retries técnicos.
3. O parser atual exige um cenário adaptativo e `runs=1` por grupo. Repetições
   desse caso usam execuções independentes de uma coorte, sem reaproveitar estado
   nem colocar `runs=5` em um manifest incompatível.
4. Resolver limites por perfil e validar contra `lane_budget`; hoje há limites
   inferidos do nome da lane, inclusive teto de 20 runs por caso e 32 casos nas
   lanes usuais. A matriz inteira exige divisão em grupos/execuções.
5. Materializar tetos de tokens, turns, concorrência, início de novos slots e
   duração por execução. O deadline de início de slots não interrompe um attempt
   já iniciado; reservar tempo próprio para avaliação, captura e cleanup.
6. Registrar primeiro attempt e retries técnicos, sem retry por resultado ruim.
   Cleanup inválido torna o attempt tecnicamente inválido e pode impedir novos
   slots em ambiente compartilhado; preservar resultados e lacunas.
7. Usar IDs de idempotência derivados de plano, perfil, coorte, stack e repetição.
   Um retry de transporte não cria outra execução; uma repetição planejada cria
   observação independente com identidade própria.

Não há teto financeiro calibrado nesta revisão. Antes de agendar os perfis,
medir consumo por caso/ambiente no piloto, estimar custo com todas as repetições
e retries permitidos e publicar preview do orçamento. Se faltar orçamento,
reduzir o perfil explicitamente ou registrar slots deferred; não publicar uma
suíte incompleta como cobertura total.

## 7. Sequência de consolidação e critérios de aceite

| Etapa | Entrega | Aceite verificável |
| --- | --- | --- |
| F0 — Unificação documental | Este plano e ponteiros dos documentos anteriores | Inventário confere com os seis manifests; rascunho preservado; todas as coberturas têm destino |
| F1 — Fonte executável única | Contrato do plano, materializador, preview e adaptação de dashboard/dispatch | Gerar primeiro os manifests atuais com paridade semântica; validar tipos, retries, limites, digests e IDs sem executar modelos |
| F2 — Medição comum | Projeção de Results v4 e eventual adapter de tasks | Reconstruir taxas/denominadores e consumo de exemplos com falha, ausência de usage, retry, cleanup inválido e execução parcial sem dupla contagem |
| F3 — Qualificação | Pilotos dos seis perfis, fixtures e verificadores qualificados | Evidência arquivada, custos observados, A/A e degradações conhecidas; casos indisponíveis e falsos alarmes reportados |
| F4 — Redução de redundância | Composição final baseada em cobertura e custo | Cada retirada preserva requisitos ou declara lacuna aceita; cenários difíceis não saem apenas porque falham |
| F5 — Migração operacional | Release Control e dashboard consomem revisões do mesmo plano | Confirmar preview, execução terminal, arquivo/restauração e comparação da mesma coorte; manter históricos e IDs anteriores |

F0 e F1 estão implementadas, incluindo exportação no dashboard e adaptação de
admissão/roundtrip da suite de dispatch. F2 está implementada para Results v4
nativo: mantém coortes separadas, recusa evidência duplicada e conserva custo de
falhas e lacunas de telemetria. Um adapter de tasks depende da integração da task
layer, ausente deste checkout. F3–F5 exigem execução e qualificação nas stacks
alvo; a agenda atual de Release Control não foi alterada. As composições novas
só substituem essa operação após qualificação. Gates de promoção continuam
dependendo de decisão própria, posterior à calibração.

## 8. Fontes de verdade para manutenção

| Assunto | Fonte |
| --- | --- |
| Fonte executável do plano | [`config/test-plan.json`](../config/test-plan.json) |
| Composição e métricas geradas | [`docs/test-profiles.generated.md`](test-profiles.generated.md) |
| Materializador e medição nativos | [`src/test_plan.rs`](../src/test_plan.rs) |
| Preview, exportação e execução | [`scripts/run_test_plan.py`](../scripts/run_test_plan.py) |
| Campanhas de compatibilidade geradas | [`config/campaigns/`](../config/campaigns/) |
| Tipos, retries, dificuldade e admissão | [`scripts/run_e2e_campaign.py`](../scripts/run_e2e_campaign.py) |
| Catálogo Rust e Markdown | [`src/scenarios/mod.rs`](../src/scenarios/mod.rs), [`scenarios/`](../scenarios/) |
| Resultado e denominadores | [`config/results-contract.json`](../config/results-contract.json), [Results v4](result-reliability.md), [`src/report.rs`](../src/report.rs) |
| Comparação e limites | [`src/longitudinal.rs`](../src/longitudinal.rs), [`src/control.rs`](../src/control.rs) |
| Planos locais do dashboard | [`src/dashboard/plans.rs`](../src/dashboard/plans.rs) |
| Arquivamento e execução exata | [`src/durable.rs`](../src/durable.rs), [`scripts/run_exact_stack_group.sh`](../scripts/run_exact_stack_group.sh) |

Os arquivos locais `config/plans/version-evolution.json`, seu ledger e
`scripts/run_version_evolution.py` foram preservados como trabalho experimental.
Eles não são o materializador deste plano: o script dirige blocos de cenários,
não executa a task layer declarada, e suas chaves/comparações precisam passar
pelos critérios acima. As observações de calibração no anexo histórico não foram
reexecutadas nem usadas para concluir causalidade sobre falhas do Harness.
