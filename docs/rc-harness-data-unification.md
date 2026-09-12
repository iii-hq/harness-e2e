# Plano de implementação: unidade de dados entre Harness E2E e Release Control

Data: 11 de setembro de 2026. Estado: preparação local implementada e validada em bancos isolados e no fluxo funcional do Console. Corte dos runtimes de uso, release e deployment ainda não aplicados. Ver seção 11.

Documento único para coordenar os dois projetos. Complementa a [reestruturação do Harness E2E](/home/layon/workspaces/harness-e2e/docs/harness-e2e-restructure.md), cuja etapa 5 já registra a implementação visual local. Esta proposta acrescenta a persistência dos planos e a importação do histórico RC, ainda ausentes naquele corte. A estratégia segue o hard refactor: contrato de destino único, migração explícita e retirada dos caminhos substituídos.

## 1. Resultado esperado

Ao importar um plano do Release Control, o Harness grava no seu banco o plano, suas execuções, configurações históricas, resultados, tentativas e referências de evidência disponíveis. A partir daí, o usuário navega pelas mesmas listas e páginas utilizadas para dados locais. O plano e suas execuções importadas recebem uma pequena identificação `remote`.

As consultas de planos, execuções, testes e métricas usam o banco local. A conexão com o RC é necessária para importar ou atualizar a cópia. Os arquivos permanecem no GitHub e são consultados sob demanda. Fechar a página do RC ou desconectar sua bridge não pode impedir a navegação pelos dados já importados.

Exemplo de aceite: importar `harness-smoke` com todo o seu histórico, abrir uma execução antiga, comparar seus resultados com uma execução local, reiniciar o Harness e repetir a navegação com o RC desconectado. Uma segunda importação atualiza a mesma identidade e acrescenta novas execuções, sem criar outro plano.

“Dump” significa uma exportação de dados do domínio que será inserida no banco de destino. O formato transportável será JSON versionado; os esquemas físicos de PostgreSQL no RC e do banco usado pelo database worker local não precisam ser iguais.

## 2. Base verificada e limites

| Projeto/cópia inspecionada | Revisão | Observação |
|---|---|---|
| Harness E2E, checkout principal | `801e7e62e7b509885b133dc50c4cc4777816eb14` e alterações locais | Reestruturação em andamento; planos ainda em arquivos e execuções operacionais em SQL |
| RC, checkout principal | `b1d42299b425fc2989c81a937d4b5831dc279077` | Não reúne todas as implementações complementares abaixo; contém alterações locais preservadas |
| RC, `feat/e2e-reference-bridge` | `c44aad1d6dee65c0e4257cb024f8e1423ebd0901` | Endpoint de referência e integração pela bridge |
| RC, `feat/activate-software-engineering-plan` | `dfdda6afdd2228dd4cf523f648816e5db62130c7` | Inclui referência, profile Software Engineering e leitura de evidências GitHub |

Essas revisões são evidência de código local, não confirmação de merge, release ou deployment. A implementação deve começar identificando a base integrada efetiva de cada projeto. Não transportar branches inteiras sobre os checkouts com trabalho em andamento.

Achados que orientam o plano:

- O [Harness materializa os profiles](/home/layon/workspaces/harness-e2e/src/test_plan.rs:275) do plano mestre. Cenários, casos, grupos, repetições e retries pertencem a essa definição.
- O [plano RC](/home/layon/workspaces/release-control-activate-se/api/src/lib/test-plans.ts:10) seleciona um profile e acrescenta modelo, juiz, executor, runner, stack e operação. O catálogo admitido ainda é mantido manualmente nos dois projetos.
- O RC já persiste campanhas, execuções, configurações congeladas, relatórios originais e resultados no [schema de dados](/home/layon/workspaces/release-control-activate-se/api/src/db/schema.ts:453). Os planos atuais são carregados de arquivos para memória.
- A [listagem RC](/home/layon/workspaces/release-control-activate-se/api/src/services/test-plans.service.ts:746) depende dos planos carregados e limita as execuções recentes. A [referência de execução](/home/layon/workspaces/release-control-activate-se/api/src/services/test-plans.service.ts:861) seleciona os relatórios mais recentes; não é um dump de todas as tentativas.
- O [relatório de materialização](/home/layon/workspaces/harness-e2e/scripts/report_execution.py:168) envia parte do snapshot. Casos completos e outros detalhes da definição não estão integralmente nesse payload.
- O [importador atual](/home/layon/workspaces/harness-e2e/src/plans/store.rs:419) cria uma reprodução local, gera outro ID e restringe os formatos executáveis. Rejeita injeção de falhas e grupos com vários cenários; não serve para importar todo o histórico.
- O [domínio local](/home/layon/workspaces/harness-e2e/src/plans.rs:19) vincula o ciclo do plano a baseline/candidatos. O [PlanStore](/home/layon/workspaces/harness-e2e/src/plans/store.rs:252) conserva planos e execuções compostas em arquivos.
- A [leitura de evidências RC](/home/layon/workspaces/release-control-activate-se/api/src/services/run-evidence.service.ts:183) já resolve bundles GitHub e valida identidade e integridade. Os [bundles do workflow](/home/layon/workspaces/harness-e2e/.github/workflows/exact-stack-e2e.yml:534) configuram retenção de 90 dias.

## 3. Decisões de domínio

### 3.1 Entidades e identidade

| Entidade | Significado comum | Regra de identidade/preservação |
|---|---|---|
| Template/profile | Composição reutilizável definida pelo Harness | ID do profile acompanhado da versão/revisão e hashes originais; o nome sozinho não identifica uma composição histórica |
| Plano | Configuração identificável, baseada em template, que possui histórico | No RC, preservar `planKey`; no Harness, preservar o ID local e a chave de origem importada |
| Snapshot da execução | Configuração e escopo efetivamente usados | Conservar modelo/juiz, materialização, casos, contratos, grupos, seeds, repetições, retries, runner e stack resolvidos |
| Execução do plano | Uma realização da configuração | Preservar ID de origem e vínculos com o plano e as unidades nativas que existirem |
| Run/slot | Caso e repetição previstos ou observados | Preservar cenário/versão, caso, seed, repetição e grupo; ausência de observação não vira nota zero |
| Tentativa | Uma tentativa em seu nível de execução | Distinguir relançamento RC, tentativa do workflow GitHub e retry técnico do run |
| Evidência | Arquivo associado à observação | Identidade, localização, hash/tamanho quando conhecidos e disponibilidade independente do resultado |

Dois planos podem compartilhar o mesmo template e avaliar modelos diferentes. Não agrupar planos pelo `template_id`, nem criar um plano novo para cada execução importada.

`test_campaigns` do RC identifica um lançamento que pode ter novas execuções. `campaigns` na materialização do Harness identifica a organização das repetições. Preservar essas relações com nomes inequívocos no contrato; não unir os campos apenas porque possuem o mesmo nome.

A execução composta local continua distinta dos filhos nativos admitidos pelo ControlPlane. A execução RC é apresentada no nível correspondente do plano e conserva seus grupos/relatórios. Não fabricar filhos locais, receipts de admissão ou bundles nativos para representar observações remotas.

### 3.2 Origem, edição e comparação

- Usar uma origem persistida e uma chave de fonte estável para deduplicação. A identidade externa deve incluir a instância RC e o ID do registro; IDs locais permanecem distintos e os IDs RC originais ficam preservados.
- `remote` é a apresentação da origem importada no Console local. O provedor de execução (`github_actions`, por exemplo) e a localização do arquivo são informações separadas. Baixar um arquivo para cache não transforma a execução em local.
- Dados importados conservam a configuração da fonte. A ação existente de reprodução passa a criar explicitamente uma configuração/execução local vinculada à referência, com diferenças registradas.
- Importação histórica não passa pelas verificações de capacidade do executor local. Essas verificações continuam na ação de executar/reproduzir.
- Baseline e candidatos são seleções ou vínculos de comparação opcionais. Migrar as escolhas existentes sem exigir um papel para toda execução RC. Não criar uma entidade persistida de comparação apenas para abrir A/B.
- Estado operacional do plano RC, estado da execução, validade técnica, resultado e disponibilidade de evidência continuam separados. Um plano pausado mantém seu histórico navegável.
- Uma execução importada ainda ativa representa o estado capturado na importação. Não entra no reconciliador, fila, cancelamento ou recuperação de trabalho local. A interface informa quando a cópia foi atualizada.
- Importar não publica dados locais no RC. Agendamento, dispatch e promoção permanecem responsabilidades do RC.

### 3.3 Autoridade dos dados

| Informação | Autoridade |
|---|---|
| Definição dos templates e materialização | Harness, vinculada à revisão do runner |
| Configuração e operação dos planos RC | RC; arquivos revisados alimentam seu catálogo persistido |
| Registro original das execuções RC | Banco RC e relatórios recebidos do runner |
| Cópia importada e navegação no Console local | Banco do Harness após a importação |
| Execuções locais | Harness e seus registros/bundles nativos |
| Bytes das evidências remotas | Artefatos GitHub referenciados pela execução |

## 4. Contrato de exportação e importação

Criar um contrato versionado de histórico E2E no Harness, reaproveitando o snapshot de profile, os resultados e o manifesto de bundle existentes. O RC implementa o produtor; o Harness implementa o consumidor. Manter fixtures comuns e verificações Rust/TypeScript/Python conforme o trecho envolvido. Não criar outro serviço nem uma biblioteca genérica para isso.

### 4.1 Conteúdo mínimo

| Bloco | Conteúdo |
|---|---|
| Envelope | Versão do schema, identidade estável da fonte, instante de captura, escopo exportado, contagens e integridade do conteúdo |
| Plano | Chave e configuração, estado/agendamento RC, referência ao template e informações de origem |
| Execuções | IDs e vínculos originais, configuração congelada, fase, tempos, erro, runner/CLI/stack resolvidos e identidade GitHub |
| Materialização | Snapshot completo quando retido, incluindo casos previstos e grupos; disponibilidade explícita quando o histórico não o possui |
| Relatórios e resultados | Todas as mensagens/tentativas retidas, hashes originais, runs observados, seleção de resultados vigente e métricas com suas unidades/completude |
| Artefatos | Referências aos arquivos e bundles, identidade do workflow/tentativa, caminhos internos, hashes/tamanhos conhecidos e disponibilidade |

Preservar os payloads originais que fundamentam o resultado. Campos ausentes em uma execução antiga permanecem ausentes com motivo; a definição atual não pode preencher silenciosamente o passado. Resultados materializados para consulta não substituem os relatórios de origem.

### 4.2 Consistência e repetição

1. Exportar um plano e todo o histórico selecionado a partir de uma leitura consistente do banco RC. Começar com uma resposta por plano, sem o limite das consultas de tela. Iterar internamente se necessário; uma seleção de vários planos produz unidades de importação independentes.
2. Usar uma transação de leitura com snapshot consistente para os registros de cada exportação. Não considerar um simples filtro por data suficiente para estabilizar linhas que podem mudar durante a leitura. Não manter a transação aberta enquanto consulta arquivos no GitHub.
3. Entregar explicitamente contagens e escopo. Erro ou interrupção de transporte não pode produzir um arquivo que pareça completo. Os endpoints de descoberta/histórico devem oferecer paginação estável, inclusive para planos retirados do catálogo ativo.
4. Validar schema, referências, integridade e identidade na entrada do worker. Gravar o plano e suas execuções/resultados em uma transação local. Uma falha não deixa metade daquela unidade visível.
5. Impor unicidade por fonte e ID de origem no banco. Reimportar conteúdo idêntico é uma operação sem alteração. Uma nova captura pode acrescentar observações e atualizar o estado da cópia da mesma execução.
6. Preservar snapshots e mensagens imutáveis: mesmo ID de mensagem com hash diferente é conflito explícito. Capturas antigas não regridem o estado de uma cópia mais recente. Definir e testar o marcador de atualização da fonte no contrato antes de implementar o upsert.
7. Ausência em uma exportação posterior não apaga dados locais. Esta etapa implementa importação explícita, sem sincronização contínua ou propagação de exclusões.

No histórico anterior ao contrato completo, exportar o que o RC realmente reteve. Se um bundle ainda disponível contém a materialização original, sua recuperação deve preservar a origem e validar a identidade. Se não há evidência, marcar a lacuna sem impedir a importação das métricas existentes.

### 4.3 Localização dos arquivos

O caminho do artefato continua relativo ao bundle. Acrescentar a localização tipada do bundle: local ou GitHub. Para GitHub, conservar repositório, workflow run ID, run attempt e identidade do artefato, mais o caminho interno do arquivo. ID do artefato, hash ou tamanho desconhecidos no histórico não devem ser inventados; registrar a referência parcial e a condição de resolução.

Reaproveitar o manifesto e as verificações de integridade existentes. Não colocar uma URL HTTP em um campo que hoje significa caminho de filesystem. Não persistir tokens, URLs assinadas temporárias ou credenciais no dump.

A navegação dos dados não exige GitHub disponível. Ao abrir evidência, resolver pela autenticação GitHub disponível no ambiente local, sem depender da aba/bridge do RC. A falta de autorização ou a expiração do bundle afeta apenas a evidência. Cache de bytes é dispensável para a primeira entrega.

Não ampliar a retenção nem criar outro armazenamento nesta etapa. Exibir separadamente arquivo ausente, expirado, acesso indisponível e integridade inválida. O resultado retido no banco permanece consultável em todos esses casos.

## 5. Mudanças no Release Control

### RC-1 — Catálogo e identidade persistente dos planos

- Persistir o plano por sua chave atual e conservar os snapshots já armazenados nas execuções. Usar a estrutura existente e uma tabela específica de planos; não criar um sistema separado de revisão quando o snapshot por execução atende ao histórico.
- Aplicar os JSON revisados ao catálogo persistido de forma explícita e idempotente. Definir os arquivos como fonte da configuração operacional, sem criar um segundo editor concorrente nesta entrega.
- Recuperar planos históricos a partir dos snapshots válidos existentes. Um plano que saiu dos arquivos fica inativo para novos disparos e permanece no histórico. Não inferir agendamento ativo de um snapshot antigo.
- Substituir a lista manual de profiles pelo catálogo versionado do Harness associado à revisão do runner selecionada. Resolver `latest` antes de validar a composição executável; conservar a revisão resolvida na execução.
- Retirar a substituição silenciosa de snapshot inválido pelo plano atual em `frozenPlan` e no relançamento. Tratar registros antigos na migração; manter eventual payload não interpretável como evidência com limitação explícita.
- Preservar agendamento, janelas, locks, idempotência, dispatch e reconciliação. Importação não altera a semântica dessas operações.

Arquivos principais: `api/src/lib/test-plans.ts`, `api/plans/`, `api/src/db/schema.ts`, `api/src/db/migrations/`, `api/src/repositories/test-plans.repository.ts`, `api/src/services/test-plans.service.ts`.

**Aceite:** dois planos usam o mesmo template com modelos diferentes; retirar um plano dos arquivos não apaga sua descoberta histórica; editar o template não muda uma execução antiga; agendamento e relançamento conservam suas garantias.

### RC-2 — Retenção do contrato completo e exportação

- Atualizar a ingestão para persistir o snapshot completo enviado pelo Harness, mantendo grupos/casos previstos mesmo quando nenhum shard produzir resultado.
- Reaproveitar `test_execution_reports` e o ledger. Exportar todas as mensagens retidas e distinguir a observação selecionada das tentativas anteriores.
- Implementar exportação autenticada por plano e paginação completa na descoberta do histórico. Compartilhar a leitura de dados com os serviços existentes; o dump não deve ser uma serialização dos componentes da UI.
- Capturar referências de bundles a partir dos manifests/relatórios e reutilizar a resolução implementada em `run-evidence.service.ts`. O manifesto deve permitir identificar arquivos além de screenshots.
- Atualizar a bridge para transportar a importação durante a operação, sem manter a bridge como fonte de consulta dos registros já importados.

Arquivos principais: `api/src/controllers/test-executions.reports.ts`, `api/src/lib/test-plan-contract.ts`, `api/src/repositories/run-ledger.repository.ts`, `api/src/services/test-plans.service.ts`, `api/src/services/run-evidence.service.ts`, controllers de planos/exportação e `app/src/lib/local-engine-bridge-functions.ts`.

**Aceite:** exportar um plano com mais de 50 execuções, várias tentativas GitHub, grupos incompletos e plano inativo; conferir contagens, vínculos e hashes com o banco da fixture. Nenhuma tentativa retida desaparece por seleção de “latest”.

### RC-3 — Navegação e contratos de apresentação

- Fazer lista, detalhe, série e comparação consultarem o catálogo persistido e os mesmos snapshots históricos.
- Mostrar configuração vigente e configuração usada em cada execução sem misturá-las. Manter detalhes de runner/stack e proveniência acessíveis na investigação.
- Oferecer exportação/importação no fluxo existente. Preservar identidade do plano, histórico e diferenças de configuração ao atravessar RC e Console.
- Alinhar unidades e completude das métricas ao contrato comum: média com peso igual por teste, ausência distinta de zero e pareamento explícito. Reutilizar o código de métricas de cada aplicação; a equivalência é verificada por fixtures comuns, sem exigir um pacote de UI compartilhado.

Arquivos principais: `app/src/lib/test-plan-types.ts`, `app/src/hooks/use-test-plans.ts`, `app/src/components/test-plans/`, `app/src/routes/_authenticated/test-plans*`.

**Aceite:** a configuração e o resultado de uma execução exportada têm o mesmo significado no RC e no Harness; diferenças de disponibilidade permanecem explícitas.

## 6. Mudanças no Harness E2E

### H-1 — Contratos e publicação do catálogo

- Definir o schema de exportação, a identidade da fonte e as fixtures que RC e Harness usarão nos testes de contrato.
- Estender o reporte para enviar o `ProfileSnapshot` completo e as referências de bundle necessárias. Atualizar schemas, Python, workflow e consumidor RC no mesmo corte de contrato.
- Publicar o catálogo derivado de `config/test-plan.json` associado à revisão/release do runner. Usar o fluxo de distribuição vigente verificado na etapa 0; não criar uma segunda definição manual dos profiles.
- Manter materialização e hashes originais no produtor. Novos identificadores do envelope não substituem hashes históricos de plano, profile, caso ou contrato.

Arquivos principais: `config/test-plan.json`, `src/test_plan.rs`, `schemas/`, `scripts/report_execution.py`, `scripts/exact_stack_campaign.py`, `.github/workflows/exact-stack-e2e.yml` e o fluxo efetivo de publicação do runner.

### H-2 — Planos e execuções compostas no banco

- Adaptar o domínio de planos para origem, histórico de execuções e comparação opcional. Reutilizar o vínculo pai/filho e os slots existentes.
- Migrar planos e execuções compostas do PlanStore para SQL por `database::query` e `database::transaction`. O database worker continua dono do acesso; não adicionar driver ou conexão direta ao Harness.
- Manter uma autoridade de escrita por entidade. Remover leitura/escrita operacional de `plan-store/plans` e `plan-store/executions` após o corte e a migração.
- Não inserir uma execução RC como trabalho admitido no ControlPlane. Usar os registros consultáveis do domínio de planos/resultados, preservando os limites entre projeção de histórico e controle de execução local.
- Adaptar artefatos/arquivos para localização local ou GitHub sem alterar o significado dos caminhos relativos nos bundles nativos.

Arquivos principais: `src/plans.rs`, `src/plans/store.rs`, `src/persistence.rs`, `src/artifact.rs`, `src/dashboard/plan_projection.rs`, `src/dashboard/read_model.rs` e consumidores do controle que dependam desses contratos.

**Aceite:** restart conserva planos, relações, baseline existente e consultas; uma execução importada ativa não é retomada nem cancelada pelo worker; uma execução composta local não duplica os totais dos filhos.

### H-3 — Importação transacional e consulta unificada

- Implementar a importação no domínio do worker, validando o envelope e persistindo o conjunto conforme a seção 4. O frontend inicia a operação e recebe contagens de inseridos, atualizados e sem alteração.
- Substituir `import_reference` pela importação histórica. Preservar a ação útil de reprodução como operação explícita separada, reutilizando criação/admissão local e suas verificações.
- Alimentar lista, detalhe, histórico de teste e comparação com os registros do banco. Reutilizar as projeções existentes sem produzir um `E2eReport` fictício para dados RC.
- Reutilizar `PrimaryMetricsView` e o contrato de métricas. Manter parcelas ausentes como indisponíveis, preservar totais válidos e não calcular deltas incompatíveis.
- Resolver evidência remota sob demanda no ambiente local. Transcritos retidos continuam no contexto E2E, sem criar ou selecionar conversas pessoais.

Arquivos principais: `src/plans/store.rs`, `src/persistence.rs`, `src/dashboard/{controller,bus,read_model,presenter,plan_projection}.rs`, `src/manifest.rs`, `dashboard/src/lib/dashboard-data-source.ts`, `dashboard/src/lib/release-control-reference.ts` e `dashboard/src/lib/primary-metrics.ts`.

**Aceite:** importar Resilience e Software Engineering mesmo sem executor local compatível; importar duas vezes sem duplicar; consultar dados com RC desconectado; ausência de arquivo não apaga resultado.

### H-4 — Uma navegação de planos e execuções

- Apresentar planos locais e importados na mesma listagem e nos mesmos detalhes; mostrar `remote` no plano e na execução importada. Templates permanecem opções para criar configurações.
- Manter o seletor de origem como filtro simples se necessário; remover a dependência de uma área RC separada para navegar pelo histórico já importado.
- Usar os mesmos links plano → execução → teste/tentativa → evidência. Comparação A/B mantém filtro, unidades e completude consistentes.
- Exibir ações conforme a capacidade: consultar/comparar importados; criar reprodução local quando aplicável. Botões locais não devem disparar cancelamento, agendamento ou edição no RC.
- Retirar `ReleaseControlPlanDetailPage` e consultas remotas de apresentação quando seus consumidores estiverem migrados. Reaproveitar apenas o transporte necessário para importar.

Arquivos principais: `dashboard/src/pages/{PlansPage,PlanDetailPage,LocalPlanPage,ExecutionsPage,ExecutionPage,ReleaseControlPlanDetailPage}.tsx`, rotas, data source e componentes de evidência.

**Aceite:** o mesmo percurso funciona para local e remoto em desktop/painel estreito, teclado e tema claro/escuro; a origem aparece sem dominar a interface.

## 7. Migração e corte coordenado

1. **Inventário:** registrar revisões, versões dos schemas, contagens, execuções ativas e alterações locais. Identificar os consumidores efetivos de referência, snapshot e arquivo. Confirmar o caminho de autenticação GitHub local que atenderá à evidência sem bridge RC.
2. **Preparação:** implementar e verificar migrações em cópias isoladas. Preservar IDs, hashes e arquivos originais; produzir relatório de dados interpretáveis, lacunas e conflitos. Usar o mecanismo `migrate-storage` existente para a evolução local, mantendo dry-run explícito.
3. **Histórico RC:** criar catálogo persistido a partir dos planos atuais e snapshots históricos válidos. Conservar payloads não interpretáveis; resolver conflitos antes de classificá-los como uma configuração válida. Falta de evidência não autoriza preenchimento pelo catálogo atual.
4. **PlanStore local:** converter arquivos autoritativos para o schema SQL de destino, preservando IDs de planos/execuções/slots, escolhas de baseline, chaves idempotentes e referências de filhos. Referências antigas criadas para reprodução continuam planos locais; importar depois a execução RC não muda a origem desses registros.
5. **Consistência do corte:** concluir ou cancelar trabalho ativo com a versão anterior antes de migrar estruturas incompatíveis. Drenar workflows/reportes antigos que não falem o contrato novo; publicar e ativar produtor e consumidor coordenadamente.
6. **Ativação:** aplicar a migração, conferir contagens/vínculos/hashes e iniciar apenas os leitores/escritores do schema de destino. Não manter escrita dupla, aliases ou desserialização que invente dados antigos.
7. **Recuperação:** usar backup verificado da base e dos arquivos anteriores com a versão de aplicação correspondente. Testar a restauração em ambiente isolado antes do corte; não tratar downgrade de binário sobre schema novo como rollback.

As migrações devem ser repetíveis e interromper diante de corrupção/conflito de identidade, sem descartar registros. Preservação do histórico é requisito de dados, não uma camada de retrocompatibilidade em runtime. A preparação local aplica migrações apenas em bancos temporários de teste; o corte dos runtimes de uso continua separado.

## 8. Sequência de entregas

| Etapa | Escopo | Dependências | Evidência de conclusão |
|---|---|---|---|
| 0 | Confirmar bases integradas, schemas, distribuição do runner e autenticação de artefatos | Nenhuma | Inventário e mapa de consumidores; WIP preservado |
| 1 | Fixar contrato de domínio/exportação e fixtures comuns | 0 | Identidades, snapshots, tentativas, captura/atualização e localizadores definidos; fixtures aceitas pelos dois projetos |
| 2 | H-1 + RC-1 + ingestão de RC-2 | 1 | Catálogo versionado, plano RC persistido e materialização completa; migração RC testada |
| 3 | H-2 + migração local | 1 | Planos no banco, origem e comparação opcionais, sem escrita em arquivos após migração |
| 4 | Exportação RC-2 + importação H-3 + resolução de evidência | 2 e 3 | Round-trip real entre bancos isolados; identidade, histórico, métricas e referências conferidos |
| 5 | RC-3 + H-4 | 4 | Navegação comum, `remote`, comparação e evidência validadas no navegador |
| 6 | Retirada dos caminhos antigos e corte coordenado | 2 a 5 | Migrações/recuperação verificadas, consumidores atualizados e aceitação com runtime conectado |

Etapas 2 e 3 podem ser implementadas independentemente depois do contrato. Dividir PRs por responsabilidade e dependência; alterações em schemas/produtores/consumidores devem identificar o corte conjunto. Quantidade de PRs não implica manter contratos antigos em produção.

## 9. Matriz de validação

| Caso | Resultado obrigatório |
|---|---|
| Dois planos do mesmo template com modelos distintos | Permanecem dois planos, com séries separadas |
| Mais de 50 execuções e plano removido do catálogo ativo | Histórico completo descoberto e exportado |
| Template ou configuração alterado depois de uma execução | Snapshot, escopo e hashes antigos permanecem iguais |
| Snapshot histórico incompleto/ausente | Dados retidos importados; lacuna explícita; nenhuma materialização atual substitui o passado |
| RC relançado, GitHub rerun e retry técnico | Identidades e relatórios preservados em seus níveis corretos |
| Teste previsto sem resultado; execução parcial ou inválida | Completude preservada; ausência não vira zero; métricas válidas continuam disponíveis |
| Resilience e grupo ordenado Registry de Software Engineering | Importação histórica funciona; reprodução aplica suas restrições próprias |
| Importação repetida ou concorrente | Um registro por chave de origem; nenhuma duplicação de totais |
| Nova captura e depois reimportação de captura antiga | Dados novos preservados; estado não regride |
| Mesmo ID imutável com hash diferente | Conflito explícito, sem sobrescrita silenciosa |
| Interrupção durante importação | Rollback da unidade; histórico anterior preservado |
| Escrita no RC durante exportação | Dados exportados internamente consistentes; contagens e referências verificáveis |
| Execução RC ativa importada | Estado capturado visível; nenhuma reserva, retomada ou cancelamento local |
| Restart, RC desconectado e bridge fechada | Planos, execuções, resultados e comparações seguem acessíveis |
| GitHub acessível, sem autorização, bundle expirado ou hash incorreto | Estado correto da evidência; métricas persistidas continuam acessíveis |
| Plano composto local com filhos nativos | Vínculos preservados e totais sem dupla contagem |
| Comparação local/remota | Mesma população declarada; média por teste; filtros simétricos; deltas apenas quando compatíveis |
| Abrir transcrito remoto | Inspeção dentro do E2E; histórico pessoal não recebe novas conversas |
| Migração repetida, arquivo corrompido e restauração | Repetição sem alterações; corrupção reportada; backup restaurável |

Executar os testes de contrato, persistência e migração em bancos temporários, incluindo o database worker real para o Harness. Reaproveitar os testes RC de loader, dispatch, ledger e evidência e os testes Rust/Python já existentes. As fixtures compartilhadas devem incluir resultados completos, incompletos e incompatíveis, com igualdade verificável das métricas entre as duas aplicações.

Para a interface, executar testes de componentes, typecheck/build e fluxos de navegador sobre os bundles efetivos. Validar pelo menos um round-trip autenticado em ambiente controlado, com dados RC retidos e GitHub real quando disponível, registrando a revisão e a disponibilidade observadas. Fixtures de UI não contam como execução real de benchmark.

Na entrega, separar: código implementado, testes locais, CI, migração aplicada, release/deployment e aceite conectado. Nenhuma dessas etapas está concluída apenas pela criação deste plano.

## 10. Limites desta implementação

- Sincronização contínua/bidirecional, publicação automática de resultados privados e edição remota pelo Harness ficam fora do escopo.
- Aumentar retenção, espelhar todos os arquivos ou criar outro serviço de armazenamento não é necessário para a importação pedida.
- Executar localmente todos os profiles do RC não é pré-condição para importar seus resultados.
- Preservar a arquitetura de chats isolados já definida; este trabalho apenas mantém a inspeção das evidências dentro do E2E.
- Reutilizar os módulos de planos, persistência, ledger, artefatos e métricas existentes. Criar tabelas/campos/endpoints apenas para requisitos concretos deste documento; remover caminhos substituídos na mesma sequência.

O trabalho estará concluído quando o banco local sustentar a navegação completa do histórico importado, com identidade e evidência preservadas, e o RC produzir esse histórico a partir do mesmo contrato de templates, planos e execuções.

## 11. Preparação local realizada em 11/09/2026

### Cópias e recorte

- Harness: `/home/layon/workspaces/harness-e2e`, branch `feat/console-metrics-ui`, base `801e7e62e7b509885b133dc50c4cc4777816eb14`. O commit desta entrega reúne a reestruturação local validada e a unificação de histórico.
- RC: `/home/layon/workspaces/release-control-data-unification`, branch `feat/rc-harness-data-unification`, criada sobre `b1d42299b425fc2989c81a937d4b5831dc279077`. Commit da entrega: `5a7292b33ba896db8bb78819ce960c7ecbb91e3d`. O checkout principal do RC não foi alterado.
- As branches `feat/e2e-session-integration` dos dois repositórios permanecem separadas e limpas. O contrato de sessões não foi transportado de volta para este checkout.
- Backup da preparação, manifesto SHA-256 e evidências de teste: `/tmp/rc-harness-unification-d2qr7zx1`. Não substitui o backup necessário para um corte de produção.

### Código preparado

| Área | Implementação local |
|---|---|
| Contrato | `schemas/e2e-history-v1.json` e `schemas/e2e-profile-snapshot-v1.json`, com cópias no RC e fixture comum de 51 execuções |
| Produtor Harness | Reporte do `ProfileSnapshot` completo e localização GitHub do bundle; nomes de artefatos vinculados ao workflow e tentativa |
| Catálogo | RC persiste planos revisados e recupera histórico inativo. Configuração não interpretável permanece indisponível, com motivo. Aplicação explícita com `bun run --cwd api test-plans:apply` |
| Profiles | RC resolve a revisão do runner e lê `config/test-plan.json` naquela revisão. O arquivo Git é a distribuição existente do catálogo; não foi criado um segundo catálogo manual nem outro workflow de release |
| Exportação RC | Descoberta paginada `release-control::test-plans::history-list` e exportação `release-control::test-plans::export`; leitura de catálogo, campanhas, execuções, reports e ledger na mesma transação PostgreSQL `REPEATABLE READ READ ONLY` |
| Persistência Harness | Storage schema 3; planos e receipts compostos em SQL pelo database worker. Migração explícita de schema 1/2, com dry-run, verificação de identidade/hash e bloqueio de execuções ativas |
| Importação | `plan-control` com `action: import_history`, transação por plano, identidade por instância/ID, preservação de reports e seleção do ledger, conflitos com rollback e ausência sem exclusão |
| Reprodução | `action: reproduce_reference` cria configuração local; `import_reference` removido. Restrições de execução local não condicionam a importação histórica |
| Console | Lista comum com `remote`, importação por arquivo/bridge, atualização explícita, execução e comparação pela cópia local; componente antigo RC retirado |
| Evidência | `e2e::dashboard::evidence-open` usa Python 3 e autenticação local do `gh`, verifica identidade do manifesto e hash/tamanho do arquivo. Retorna `available`, `missing`, `expired`, `access_unavailable` ou `integrity_invalid` |

### Regras do contrato fixadas

O transporte é `{ "json": "<JSON UTF-8 exato>", "sha256": "sha256:<hex>" }`. O checksum cobre os bytes da string antes da desserialização. A importação de arquivo preserva esse envelope; para JSON de domínio diretamente, calcula o checksum com o mesmo prefixo. Hashes originais de reports/runs permanecem preservados, separados dos checksums locais de armazenamento.

A revisão do plano usa o `updatedAt` do catálogo; seu hash cobre configuração, estado ativo e limitação. A revisão da execução é o maior marcador entre a execução e os reports/runs retidos. O ledger selecionado usa `capturedAt` do run. `captured_at` do envelope informa a captura e não autoriza sobrescrever uma revisão igual. Uma revisão menor não regride os dados; revisão igual com conteúdo conflitante falha. Reports são imutáveis; a seleção do ledger pode avançar e continua apontando para o report original retido. Aumentar apenas o instante da captura não altera a cópia.

Histórico antigo sem snapshot completo continua importável. O RC valida o snapshot pelo schema gerado no Harness e classifica payloads parciais explicitamente. Nenhum dado é preenchido a partir do profile atual. Os localizadores GitHub podem ser parciais porque o reporte antecede o upload; ID, tamanho e checksum desconhecidos permanecem nulos.

### Validação realizada

- **Rust:** 690 testes da biblioteca passaram; 5 testes dependentes de ambiente ficaram ignorados nessa execução. Os quatro testes novos de banco real foram executados separadamente e passaram. Clippy com `-D warnings` e build do binário passaram.
- **Database worker real:** importação de 51 execuções, reimportação concorrente/idempotente, captura antiga, conflito/rollback, retenção após omissão, isolamento de trabalho remoto, migração vazia e migração com baseline/candidatos/slots. Arquivo corrompido bloqueou a aplicação sem criar os registros de destino.
- **PostgreSQL real temporário:** migrações RC, catálogo, dispatch, settle, ingestão de reports e exportação. A saída real do produtor foi consumida pelo teste Rust via database worker, preservando 51 execuções, 3 reports, um run selecionado, seed em string acima do limite inteiro seguro de JavaScript, score `0.125`, 100 tokens e telemetria ausente.
- **Binário iniciado no engine isolado:** `plan-control`, `plans-list`, `plan-get`, `executions-list` e `execution-get` funcionaram. Reiniciar o processo conservou plano e detalhe idênticos, sem bridge RC registrada e sem diretório `plan-store`. Criação, leitura, edição e exclusão de plano local também passaram pelo SQL em runtime.
- **Python:** 48 testes de reporte, contrato/empacotamento e evidência passaram. Os testes de evidência verificam arquivo não visual, identidade incorreta, corrupção, traversal, ausência, expiração e indisponibilidade de autenticação; não representam download de artefato GitHub real.
- **Console:** typecheck, 261 testes, build e fluxo Playwright passaram. O navegador importou o transporte compartilhado, abriu a execução, recarregou com RC desconectado e abriu comparação local/remota em Test History sem novos RPCs RC.
- **RC frontend/API:** typechecks, metadados das migrações e suites focadas de catálogo, loader, controllers, dispatch, ledger, reports, exportação e bridge passaram. As suites que usam PostgreSQL criam e removem bancos temporários; os testes da bridge simulam HTTP.

Os testes Rust de contrato real são opt-in: `HARNESS_E2E_TEST_DATABASE_URL` deve apontar somente para um engine/database worker isolado. `HISTORY_EXPORT_ARTIFACT` indica a saída da integração PostgreSQL do RC. Executar `cargo test --lib real_database -- --ignored --test-threads=1`; os testes de migração recriam tabelas da base de teste e não devem receber uma base em uso.

### Corte e limites ainda separados

Esta entrega possui commits locais. Não houve push, CI remoto, publicação, deployment ou migração da base em uso nesta preparação. Antes da ativação conjunta, ainda é necessário drenar trabalho ativo, aplicar as migrações com backup, configurar uma identidade RC estável em `RELEASE_CONTROL_INSTANCE_ID`, aplicar o catálogo e validar o acesso aos artefatos GitHub no ambiente de destino.

O teste de navegador usa o host de RPC da extensão, com funções simuladas; comprova o fluxo funcional e a independência da bridge RC. As capturas desse host não comprovam os temas e o layout do Console real. Aceite visual no Console integrado, round-trip autenticado no ambiente de destino e restauração do backup desse ambiente continuam na etapa 6.

A listagem local já pagina a resposta, mas a projeção ainda carrega o conjunto retido antes de aplicar a página. A preparação foi validada com 51 execuções; não há resultado de carga para históricos grandes. Consultas SQL limitadas por página são o ponto identificado para essa validação de escala, sem alterar o contrato externo.
