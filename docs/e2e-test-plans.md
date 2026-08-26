# Planos de testes E2E

Os planos canônicos são `daily`, `weekly` e `post-release`. Não existe plano de
PR. Todos permanecem advisory enquanto o histórico longitudinal é calibrado:
falha objetiva, infraestrutura, cobertura, orçamento e nota continuam sinais
separados.

## Como interpretar a nota

Cada execução recebe uma nota de 0 a 100 formada por critérios determinísticos.
Critérios hard gate decidem se o cenário passou; critérios advisory refinam a
qualidade sem converter o plano em gate de promoção. Quando há repetições, o
Harness publica taxa de aprovação e mediana das notas, além de preservar cada
execução individual.

A dificuldade não é um multiplicador cosmético. Ela aparece na nota porque os
pontos de maior peso exigem evidência de código: reprodução do defeito, patch
limitado, testes públicos, probes ocultos, integridade do repositório e execução
real. Os casos mais difíceis usam um cohort canônico próprio, identificado pelo
seed e pelo hash dos inputs, sem incrementar `scenario_version`.

## Plano diário

Objetivo: detectar regressões rapidamente, mas manter código como o principal
sinal. Cada cenário roda uma vez e o grupo aceita uma repetição somente para
falha técnica de infraestrutura.

- `minimal_path` — Mede o caminho mínimo e o custo básico do Harness. O agente
  grava um valor exato no estado e responde de forma curta. O avaliador confere
  valor, número de chamadas, turnos e atrito para detectar aumento de overhead.

- `tool_contract_recovery` — Testa recuperação diante de um runbook com uma
  ferramenta antiga. O agente deve descobrir o contrato atual, buscar perfil e
  timezone, migrar para a função v2 e criar o evento sem chamar a operação
  destrutiva que existe como decoy. Transcript, audit log e estado final são
  comparados pelo runner.

- `shell_coder_sandbox` — Testa uma correção de código completa e reproduzível.
  O agente recebe uma implementação Python defeituosa de reconciliação de
  eventos. Código inicial, testes públicos e tarefa vêm do subtree
  `shell-coder-sandbox` de um commit imutável de `iii-hq/e2e-fixture`; o harness
  valida SHA do commit, topologia e digest do subtree antes de usá-los. O agente
  deve ler os três assets, reproduzir a suíte vermelha, registrar um diagnóstico,
  alterar somente o arquivo de produção, deixar os testes públicos verdes e
  passar probes ocultos de revisões fora de ordem, conflito, validação,
  imutabilidade da entrada e contas com saldo zero. Depois, deve provar o mesmo
  comportamento no host e em um sandbox Python sem rede, copiar exatamente
  código e testes e encerrar o sandbox.

- `performance_regression` — Testa otimização sem regressão funcional. O agente
  recebe uma função Python correta, porém quadrática, e deve reduzir o trabalho
  algorítmico mantendo a ordem e o resultado. O runner usa testes públicos e
  ocultos e contadores determinísticos de hash/igualdade; tempo de parede é
  apenas sinal advisory.

- `database_migration_recovery` — Testa conclusão segura de uma migração
  interrompida. A base já contém schema v2 e uma cópia parcial. O agente deve
  terminar o backfill, colocar o registro inválido em quarentena, criar a view de
  compatibilidade e repetir a transação sem duplicar dados nem tocar o sentinel.

- `engineering_ticket_git_handoff` — Testa engenharia coordenada por Git. A
  sessão raiz delega planejamento e implementação a sessões diferentes; o plano
  e o patch precisam virar checkpoints lineares. O runner reproduz o baseline,
  executa testes focados, ocultos e completos, valida ancestralidade, escopo,
  ausência de merge e encerramento das sessões.

- `chess_engine_build` — Testa construção algorítmica difícil. O agente implementa
  `legal_moves` e `perft` em um engine Python incompleto, incluindo roque, en
  passant, promoções, pins e evasão de cheque. O runner compara movimentos e
  contagens de nós contra um oracle independente em várias posições.

- `git_regression_forensics` — Testa investigação sobre histórico real. O agente
  clona um bundle Git offline com 500 commits, reproduz os endpoints bom e ruim,
  encontra o primeiro commit defeituoso com busca eficiente e produz relatório
  estruturado. O runner verifica o limite pass/fail, os SHAs, caminhos citados e
  a quantidade de probes executados.

## Plano semanal

Objetivo: medir repetibilidade, concorrência, coordenação e capacidade longa. A
suíte principal roda aos domingos; fault recovery e endurance são trilhas
semanais especializadas do mesmo plano.

- `minimal_path` e `timer_wake` — Canário único. Além do custo mínimo, verifica
  que um timer relativo é armado antes da escrita, acorda a sessão no momento
  correto e não deixa recursos ativos.

- `shell_coder_sandbox` — Três execuções. Mede se investigação, patch e paridade
  do sandbox convergem de forma repetível, não apenas se um caso isolado passou.

- `performance_regression` — Cinco execuções. É o principal sinal estatístico de
  regressão de código e compara taxa de aprovação, mediana da nota e dispersão
  do trabalho determinístico.

- `chess_engine_build`, `git_regression_forensics` e
  `engineering_ticket_git_handoff` — Três execuções de cada. Cobrem, respectivamente,
  implementação algorítmica, diagnóstico em histórico real e colaboração entre
  sessões com checkpoints Git.

- `contention_ledger` — Três execuções. Três writers paralelos fazem cinco
  incrementos atômicos cada sobre o mesmo acumulador. O runner exige total 15,
  quinze registros de auditoria, wake armado antes do fan-out e ausência de lost
  update.

- `security_review` — Uma execução composite. Exercita duas revisões exatas de
  um repositório local, deduplicação de scans, detecção de capacidades de
  segurança, sugestões aplicáveis sem mutação e reconciliação do ciclo com
  GitHub.

- `incident_response` — Uma execução adaptativa. Uma evidência determinística
  invalida a hipótese inicial sobre duplicação do provedor. O agente deve
  replanejar, demonstrar que o problema é redelivery após timeout de ACK e
  escolher remediação ou rollback sem executar ambos.

- `cross_repo_contract_migration` — Uma execução adaptativa. O agente migra
  produtor e consumidor visível; um canário revela depois um segundo consumidor
  incompatível. O segundo plano deve preservar trabalho aceito, corrigir os três
  repositórios e passar contratos antigos e novos.

- `fault_recovery_matrix` — Um único cenário lógico de recuperação, executado em
  três fases calibradas. A fase stateful injeta atraso, primeira escrita falha e
  duplicação; a fase de coordenação acrescenta falha/timeout de filho e resultados
  fora de ordem; a fase coordenada avançada combina falha de ramo, duplicação e
  reordenação. Cada fase exige resultado correto, árvore íntegra, cleanup completo
  e amplificação dentro do teto de 2x, 3x e 3,5x. Por padrão são ao menos três
  repetições e 60 minutos por fase, mas o time apresenta e acompanha uma única
  capacidade: recuperar trabalho stateful/coordenado sem duplicar efeitos.

- `engineering_endurance_ladder` — Uma execução longa e progressiva. Uma única
  sessão evolui uma fila durável por até dez tickets cumulativos, com checkpoints
  Git, testes públicos e probes ocultos em cada degrau. A métrica principal é o
  maior degrau aceito antes do limite de capacidade; atingir o limite é resultado
  válido, não falha de infraestrutura.

## Plano pós-release

Objetivo: validar a versão publicada pelo caminho de consumo e recuperação que
mais se aproxima do uso real. Todos os cenários rodam uma vez, sem retry técnico,
para não mascarar uma regressão da release.

- `minimal_path` e `tool_contract_recovery` — Confirmam disponibilidade básica,
  eficiência e descoberta do contrato de ferramentas na versão publicada.

- `shell_coder_sandbox`, `performance_regression`, `chess_engine_build` e
  `engineering_ticket_git_handoff` — Formam a bateria de código: corrigir,
  otimizar, implementar algoritmo complexo e coordenar uma entrega por Git.

- `database_migration_recovery` — Confirma idempotência e preservação de dados
  diante de estado parcialmente migrado.

- `cross_app_transaction` — Converge uma conta em três serviços versionados e
  recupera de um conflito CAS determinístico. O runner lê diretamente snapshots
  e audit logs, sem confiar apenas no texto final do agente.

- `browser_cross_site` — Usa navegador real em três origins locais isoladas. O
  resultado é conferido contra estado de backend controlado pelo runner para
  detectar falhas de navegação, sessão ou isolamento entre sites.

- `release_train_recovery` — Recupera uma release imutável cancelada depois de
  assets parciais, reutilizando o mesmo run id em nova tentativa. Em seguida, uma
  incompatibilidade no `latest` invalida o plano: o agente deve preservar o
  ponteiro real, criar uma nova operação gated e comprovar a versão exata antes
  da promoção, sem retag, bump ou mutação direta do canal.

## Leitura especial da nota de `shell_coder_sandbox`

O cenário mantém `scenario_version = 5`, mas usa o cohort
`code-hard-2026-08`. Os assets visíveis não vivem no binário do harness: são
carregados do subtree `shell-coder-sandbox` do repositório
`iii-hq/e2e-fixture`, fixado por commit e por digest de conteúdo. Os probes
ocultos permanecem no runner, para que o fixture público não revele
integralmente a avaliação.

Os 100 pontos agora são distribuídos assim:

- 5: workers necessários disponíveis e instalados;
- 15: leitura dos três artefatos e reprodução vermelha antes do primeiro edit;
- 5: diagnóstico escrito e movido para o local final;
- 20: suíte pública aceita pelo runner;
- 25: probes ocultos aceitos pelo runner;
- 10: demo correta no host;
- 15: mesma fonte, mesmos testes e mesmos resultados no sandbox sem rede;
- 5: escopo exato, ordem das evidências e sandbox encerrado.

Assim, uma execução que apenas chama ferramentas corretamente não obtém uma boa
nota. A maior parte dos pontos depende de produzir código correto e evidência
independente de que o comportamento se mantém fora do contexto em que o patch
foi escrito.
