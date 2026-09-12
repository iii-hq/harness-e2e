# Contrato das métricas E2E no Console — v1

Data: 2026-09-09. Estado: primeira implementação local nas telas de execução e plano do Console, na branch `feat/console-metrics-ui`; ainda não publicada no worker.

Atualização de 2026-09-11: o [fluxo de referência da etapa 4](../harness-e2e-restructure.md#14-etapa-4-fluxo-visual-de-referência) alterna Grouped / By test no mesmo componente e oferece transcritos por lado, teste e versão. O contrato de cálculo abaixo permanece vigente; a serialização do filtro no link continua pendente. A [etapa 5](../harness-e2e-restructure.md#15-etapa-5-console-como-interface-única) estende o componente às referências RC, com pareamento e limites de disponibilidade próprios do ledger.

## 1. Escopo e decisões

Definido com o usuário:

- O Console é o destino da interface. Adotar iii Schematic e os componentes públicos de `@iii-dev/console-ui`.
- A primeira visão é de métricas: input/output de tokens, normal/cache, turnos, function calls, function errors, tempo, nota da avaliação e valor gasto.
- Uma execução de plano apresenta primeiro o agrupado e depois as mesmas métricas por teste. A comparação de planos repete essa estrutura, com A, B e diferença.
- A nota do plano é a média das notas dos testes, com peso igual por teste.
- O usuário pode ocultar testes com nota zero ou resultado ausente na comparação. Se qualquer lado se enquadrar, remover o teste dos dois lados e recalcular o recorte inteiro.
- Juiz, critérios booleanos, pass rate e painéis de investigação não compõem a primeira visão. O estado operacional pode aparecer discretamente na identidade da execução.
- Explicação da avaliação, mensagens, chamadas individuais, eventos e artefatos são uma segunda etapa, fora destes wireframes.

Escolhas de elaboração usadas nesta v1, ainda não confirmadas individualmente:

- Repetições: média aritmética das notas finais dos runs de um teste; consumo somado.
- Métrica principal de tempo: **Tempo acumulado**, somando as durações dos runs, inclusive retries. Ela é recalculável no mesmo recorte dos demais contadores. Tempo decorrido do plano é um contexto separado, não substitui essa soma e não recebe delta sobre um subconjunto filtrado.
- Filtro de zero/ausente inicialmente desligado; ativação explícita pelo usuário. O estado deve acompanhar o link de comparação.
- Nota em escala 0–100; gasto em USD, conforme o relatório, sem conversão monetária ou reconstrução por tabela de preços.

## 2. Unidade de observação e população

**Tentativa física:** uma tentativa real de executar um run. **Run lógico:** uma repetição planejada, com uma nota final e consumo de todas as suas tentativas. **Teste:** unidade identificada pelo teste, versão e conjunto de casos do plano. **Execução de plano:** conjunto desses testes em uma execução identificada.

Contar subagentes no run que os originou. Não somar outra vez suas sessões se a telemetria do run já agrega a árvore. Retries contribuem para consumo, tempo e contadores exatamente uma vez; a nota usa a avaliação final do run, sem tratar cada retry como uma nova repetição.

Todos os totais usam a mesma população de testes selecionada. Cada métrica mantém sua própria disponibilidade: um teste com nota disponível e custo ausente continua contribuindo para a nota e exibe custo indisponível. Não retirar silenciosamente testes de apenas uma métrica.

## 3. Dicionário de métricas

Os identificadores abaixo descrevem a normalização necessária para a apresentação; não declaram uma nova API. Contadores usam inteiros exatos, duração usa milissegundos e custo preserva a precisão informada até a formatação.

| Identificador | Rótulo | Significado e regra |
| --- | --- | --- |
| `input_normal` | Input normal | Tokens de entrada sem cache. Parcela disjunta de cache lido e cache escrito. |
| `cache_read` | Cache lido | Tokens de entrada reutilizados de cache. |
| `cache_write` | Cache escrito | Tokens de entrada contabilizados na criação de cache. |
| `input_total` | Input total | `input_normal + cache_read + cache_write`, somente depois de normalizar a semântica da fonte. |
| `output_normal` | Output | Tokens gerados. Reasoning, quando já incluído em output, não é somado de novo. |
| `turns` | Turnos | Contador oficial da execução, incluindo a árvore de subagentes e todas as tentativas. No runtime atual, cada mensagem de assistant conta como um turno; a projeção inclusiva é `root_turns + child_turns`. Não reconstruir esse contador no frontend nem contar chunks ou function calls como turnos adicionais. |
| `function_calls` | Function calls | Chamadas registradas na execução, incluindo chamadas que retornaram erro. |
| `function_errors` | Function errors | Chamadas com erro conforme o contador oficial. Não é contagem de linhas de log ou de erros genéricos da execução. |
| `duration_ms` | Tempo acumulado | Soma da duração dos runs lógicos, cada um já incluindo suas tentativas. Não somar durações de filhos paralelos à duração do run pai. |
| `score` | Nota da avaliação | `run.objective_score`: nota final numérica registrada na avaliação. Não inferir nota a partir de status, sucesso técnico, presença de artefato ou critério booleano. Outras auditorias não substituem essa nota. |
| `cost_usd` | Valor gasto | Custo registrado pelo runtime para a execução, incluindo subagentes e retries. Pode ser informado pelo provider ou estimado pelo runtime; identificar estimativa e cobertura quando aplicável. O frontend não reconstrói custo a partir dos tokens. Sem completude comprovada, não apresentar como gasto total confirmado. |

Input total e suas três parcelas ficam visíveis juntos; output é uma métrica própria. O contrato atual possui cache lido/escrito e não declara uma parcela de “output em cache”. A interface não inventa essa categoria. Um total geral de tokens, se necessário em contexto secundário, é `input_total + output_normal` e nunca adiciona novamente o cache.

**Semântica de cache:** algumas fontes entregam input já inclusivo; outras entregam input normal separado do cache. A normalização precisa conhecer a convenção da fonte. Quando o input é inclusivo, subtrair apenas parcelas que a fonte documenta como incluídas; quando é exclusivo, conservar o input como normal. Não deduzir a convenção pela magnitude dos números. Sem convenção comprovada, a decomposição é indisponível, com motivo; não fabricar um input normal.

No pipeline consultado, os adapters do llm-router já normalizam `Usage.input` como entrada normal e preservam `cache_read`/`cache_write` separados. Portanto, na projeção Harness, **não subtrair cache novamente**. Input total requer as três parcelas conhecidas. Uma decomposição inconsistente não é uma medição válida de zero.

## 4. Agregação

### Consumo e contadores

Para tokens, turnos, calls, errors, tempo acumulado e custo:

1. Obter o valor inclusivo de cada run lógico, sem duplicar retries ou subagentes.
2. Somar os runs de cada teste.
3. Somar os testes do recorte para o agrupado do plano.

O tempo acumulado pode superar o tempo decorrido em planos com testes paralelos. O rótulo deve conservar essa distinção. Uma comparação de consumo exige escopo e repetições compatíveis; mais repetições não podem parecer uma regressão de consumo sem contexto.

### Nota

Para um teste `t` com `n` runs esperados e todas as notas disponíveis:

`nota_teste(t) = soma(notas_finais_dos_runs) / n`

Para um recorte `S` de testes com notas disponíveis:

`nota_plano(S) = soma(notas_dos_testes_em_S) / quantidade_de_testes_em_S`

Não calcular a média do plano juntando diretamente todos os runs: isso daria mais peso a testes com mais repetições. Exemplo: teste X com notas `[100, 0]` tem nota 50; teste Y com `[100]` tem nota 100; o plano tem nota **75**, não 66,67.

Se falta uma nota final de repetição esperada, a nota agregada desse teste é indisponível. Não reduzir o denominador silenciosamente. O agrupado pode apresentar **média observada**, com `N/M testes com nota`, sem tratá-la como nota completa. Zero medido entra normalmente na média quando o filtro está desligado.

## 5. Ausência, parcialidade e estados

Cada métrica deve distinguir valor completo, subtotal/média observada e indisponibilidade. Isso pode ser projetado a partir dos dados existentes; não exige um novo conjunto de wrappers.

- **Zero:** dado medido, renderizar `0`.
- **Sem qualquer medição:** renderizar `—` e uma indicação curta do motivo.
- **Algumas medições ausentes:** mostrar o subtotal observado com `parcial · N/M runs` ou `N/M testes`, com a unidade do denominador explícita.
- **Nota parcial do plano:** mostrar a média observada e `N/M testes com nota`; a tabela identifica quais testes estão sem nota.
- **Carregando:** manter a estrutura e usar placeholders. Não apresentar temporariamente “sem dados” ou valores zero.
- **Em andamento:** valores observados são provisórios. O denominador é o escopo esperado, e os deltas terminais ficam indisponíveis enquanto o par não estiver finalizado.
- **Erro de carregamento:** erro localizado na superfície que falhou; não substituir valores ausentes por zero.

O subtotal só usa medições oficiais completas das unidades conhecidas. Se o runtime invalida o total de um run por falta de telemetria, esse run permanece indisponível para aquela métrica; não recuperar um subtotal de mensagens ou gerações e apresentá-lo como total do run. Cobertura de custo e sua origem são dimensões separadas: uma estimativa pode estar completa, enquanto um valor numérico sem cobertura conhecida não prova totalidade.

Números abreviados são apenas formatação; os cálculos usam valores completos. Exibir precisão útil, sem arredondar gasto positivo pequeno para zero. Manter valor exato acessível por foco/toque, sem depender exclusivamente de hover.

## 6. Comparação de planos

### Identidade e conjunto comum

Os lados A e B são execuções explícitas de plano. A identidade do teste inclui versão e casos; nomes iguais não bastam. Mudanças de identidade incompatíveis ficam visíveis e identificadas, sem delta controlado. Mudanças de modelo ou stack podem ser a variável da comparação: não exigir igualdade justamente da variável que A/B se propõe a comparar. Explicitar o contexto e controlar as demais condições relevantes, incluindo repetições e política de execução.

Sem filtro, resumo e tabela incluem a união dos testes dos dois lados. Um teste sem resultado em um lado mantém esse lado parcial; não reduzir silenciosamente o denominador ao conjunto comum. Com o filtro ativo, resumo e tabela usam exatamente o mesmo recorte simétrico. Incompatibilidade de casos, contratos, política ou repetições bloqueia os deltas, sem esconder os valores medidos. Uma linha compatível pode manter seu delta mesmo quando o agregado não permite comparação. Mostrar a quantidade incluída e os motivos de exclusão junto do recorte, sem ocupar a área principal com detalhes técnicos.

### Filtro opcional

Rótulo: **Ocultar testes com nota zero ou sem resultado**.

Para cada teste da união dos dois lados:

`incluir = resultado_A_disponível && resultado_B_disponível && nota_A > 0 && nota_B > 0`

Resultado disponível inclui nota final agregada disponível. Se a condição falhar, excluir o teste inteiro de **A e B**, de todas as métricas, da média do plano e da tabela principal. Não manter o consumo de um teste excluído em um dos totais.

- Nota A=80, B=0: excluir os dois lados.
- Nota A ausente, B=90: excluir os dois lados.
- Notas positivas em ambos, custo B ausente: manter o teste; custo B continua parcial/indisponível e seu delta fica indisponível.
- A=[0,100] → nota do teste 50, B=[80,80] → nota 80: manter o teste. O filtro atua sobre a nota agregada do teste, sem remover repetições isoladas.
- Nenhum teste elegível: mostrar “Nenhum teste com resultado positivo em ambos os lados”, totais e notas `—`, sem divisão por zero.

Exibir `2 de 4 testes incluídos · 2 ocultos`, com motivos consultáveis em uma seção discreta. A seleção pode ser desfeita e não altera relatórios originais. O filtro descreve um subconjunto com notas positivas; não transforma uma nota positiva em um novo critério booleano de sucesso.

### Deltas

- Diferença absoluta: `B − A`.
- Nota: diferença em **pontos**, não percentual relativo.
- Demais métricas: diferença absoluta; percentual opcional `(B − A) / A × 100` quando A>0.
- A=0 e B disponível: mostrar diferença absoluta; percentual `—`. A=0/B=0 tem diferença absoluta 0.
- Se uma métrica estiver parcial/ausente em qualquer lado, seu delta agregado é `—`; valores observados e cobertura continuam visíveis. Contagens N/M iguais não provam cobertura da mesma população.
- A linha de um teste completo pode ter delta mesmo que outro teste torne o agrupado parcial.
- Sinais e setas representam direção numérica. Menos tokens, tempo ou gasto não recebem automaticamente rótulo de “melhor”; a nota permanece visível junto do consumo.

## 7. Composição dos dois wireframes

**Execução de plano:** cabeçalho compacto com identidade → métricas agrupadas → mesmas métricas por teste. Input inclui normal/cache lido/cache escrito à vista; turnos, calls, errors, tempo, nota e gasto permanecem visíveis. Nenhum seletor de métrica deve esconder dimensões essenciais.

**Comparação de planos:** identidade de A e B → filtro e indicação do recorte → métricas agrupadas com A/B/diferença → mesmas métricas por teste. Acionar o filtro atualiza resumo e tabela juntos. Motivos de exclusão ficam em disclosure separado.

Em painel estreito, reordenar as células em grupos legíveis, conservando rótulos e todas as métricas. Scroll horizontal somente dentro de uma tabela cuja composição o exija; não encolher a tipografia nem cortar números. Na implementação, validar painel dividido real, temas claro/escuro, teclado e toque.

Usar estrutura, tabelas, badges, disclosures e controles do Console. Adaptar componentes de domínio existentes; não criar outra biblioteca de botões, campos ou diálogos. Nesta fase não incluir prompts, matrizes de critérios, logs ou painéis técnicos na primeira visão.

## 8. Exemplo verificável

Arquivo: [console-metrics-v1.examples.json](./console-metrics-v1.examples.json). Todos os valores são demonstrativos e já normalizados; não representam resultados reais nem validam o pipeline atual.

Quatro testes: `minimal_path`, `persistent_state`, `contention_ledger`, `timer_wake`. Em B, `contention_ledger` tem nota zero e `timer_wake` não tem resultado.

- Sem filtro: A tem nota 81,75 com 4/4 testes; B tem média observada 62 com 3/4 testes. O delta agregado da nota fica indisponível.
- Com filtro: ficam `minimal_path` e `persistent_state`, igualmente nos dois lados.

| Métrica filtrada | A | B | B − A |
| --- | ---: | ---: | ---: |
| Nota | 86 | 93 | +7 pontos |
| Input normal | 30.000 | 26.000 | −4.000 |
| Cache lido | 80.000 | 95.000 | +15.000 |
| Cache escrito | 11.000 | 9.000 | −2.000 |
| Input total | 121.000 | 130.000 | +9.000 |
| Output | 6.000 | 5.000 | −1.000 |
| Turnos | 20 | 16 | −4 |
| Function calls | 50 | 41 | −9 |
| Function errors | 3 | 1 | −2 |
| Tempo acumulado | 5m | 4m | −1m |
| Valor gasto | US$ 0,12 | US$ 0,10 | −US$ 0,02 |

## 9. Fontes existentes e lacunas para implementação

Esta seção descreve o checkout consultado, não modifica as decisões de produto acima. Campos e rótulos legados ainda presentes no código não são requisitos desta interface.

- [Contrato de métricas da UI](/home/layon/workspaces/harness-e2e/dashboard/src/lib/dashboard-data-source.ts:312): declara input/output, cache read/write, reasoning, turns e function calls/errors. O resumo atual não consolida todas essas parcelas.
- [Telemetria do runtime](/home/layon/workspaces/workers/harness/src/functions/metrics.rs:323): preserva input normal, output, cache lido/escrito e reasoning separadamente; invalida o total de um campo se uma geração não o informar. Essa indisponibilidade não deve ser substituída por somas parciais de transcript.
- [Relatório de eficiência](/home/layon/workspaces/harness-e2e/src/report.rs:262): possui turns da raiz e filhos, contadores, input/output, duração e custo. `total_tokens` atualmente soma somente input normal e output, excluindo cache. Cache lido/escrito precisam integrar a projeção; turnos precisam expor a soma de raiz e filhos.
- [Consolidação de retries](/home/layon/workspaces/harness-e2e/src/report.rs:941): `attach_retry_attempts` e `aggregate_retry_efficiency` já incluem tentativas anteriores uma vez em tempo, custo e contadores. As parcelas de cache devem seguir a mesma regra, sem adicioná-las duas vezes. O tempo resultante já é acumulado, não o tempo decorrido global do plano.
- [Nota canônica](/home/layon/workspaces/harness-e2e/src/report.rs:649): usar `objective_score`, preservando zero e ausência. Não substituir por `validation_score` nem `instruction_adherence.score`.
- [Agregação atual](/home/layon/workspaces/harness-e2e/dashboard/src/lib/execution-metrics.ts:134): usa medianas de scores e não fornece toda a decomposição requerida. A v1 pede média por teste e média entre testes; isso exige mudança explícita, não somente novo rótulo.
- [Comparação atual](/home/layon/workspaces/harness-e2e/dashboard/src/lib/plan-comparison.ts:29): precisa expor a mesma normalização e o mesmo recorte do resumo e da tabela, incluindo as parcelas de cache.
- [Origem de custo](/home/layon/workspaces/workers/llm-router/src/chat/pricing.rs:7): o runtime pode usar custo do provider ou estimar por tokens e preços conhecidos. O relatório precisa qualificar origem e completude antes de a UI garantir totalidade. Não cabe ao wireframe nem ao frontend corrigir preços ou preencher parcelas ausentes.
- [Normalização de input](/home/layon/workspaces/workers/provider-openai/src/sse.rs:174): o adapter separa cache do input total. O uso atual de subtração saturada pode esconder inconsistências como zero; essa é uma lacuna de validação na origem, a tratar antes de prometer decomposição exata para qualquer payload.
- [Componentes públicos do Console](/home/layon/workspaces/workers/packages/console-ui/README.md): fonte da biblioteca compartilhada. Os wireframes representam a composição, sem instalar outro runtime de componentes no worker.

A implementação usa [primary-metrics.ts](../../dashboard/src/lib/primary-metrics.ts) nas duas telas e preserva essas limitações da fonte: não converte relatórios antigos para o contrato atual nem preenche parcelas ausentes. O custo é apresentado como gasto registrado pelo runtime.

## 10. Aceite da implementação

- Todas as métricas acordadas aparecem no agrupado e por teste, na mesma ordem e unidade.
- Média por teste tem peso igual no plano; repetições não alteram esse peso.
- Cache, retries e subagentes não são contados duas vezes.
- Zero, ausente, parcial e carregando são estados distintos.
- Filtro remove simetricamente teste zero/ausente, recalcula resumo e tabela e pode ser desfeito.
- Deltas usam o mesmo conjunto e não alegam eficiência quando os dados não permitem.
- Tempo acumulado é rotulado como tal; tempo decorrido não é fabricado a partir de somas.
- UI usa o sistema do Console, funciona em painel estreito e mantém investigação fora da primeira visão.

## 11. Validação local

- Projeção coberta por testes de média por teste, retries, ausência, filtro simétrico, contratos, escopo inconsistente e limites numéricos.
- Execução real com nove testes validada no Console: média 96,89; cache escrito ausente mantém input inclusivo indisponível, com normal e cache lido visíveis.
- Comparação com os dados demonstrativos acima: filtro 4 → 2 → 4, notas 86 e 93 após o recorte e delta +7 pontos.
- Navegador validado em 1440, 720 e 390 px nos temas claro e escuro, incluindo teclado, painel de 360 px, filtro vazio recuperável e preservação de Execution only durante atualizações simuladas.
- Nenhuma execução de benchmark foi iniciada nesta validação. Repetições e retries foram exercitados em testes unitários; o artefato real consultado possui uma repetição por teste, sem retries.
