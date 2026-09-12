# Setup sem configuração manual do Harness E2E

Status: primeiro incremento implementado localmente; etapas restantes propostas. Auditoria de 12/09/2026 sobre a árvore local de `feat/console-metrics-ui`, baseada em `61a4a33`, incluindo as alterações locais existentes. O inventário abaixo registra a situação encontrada na auditoria.

## Primeiro incremento

- Worker e `migrate-storage` usam o mesmo YAML, por `--config`/`III_CONFIG`. A migração deriva banco, namespace e diretório de evidências dessa configuração; caminhos relativos são resolvidos a partir do arquivo YAML. Foram removidos os overrides `HARNESS_E2E_CONTROL_DATABASE`/`HARNESS_E2E_CONTROL_NAMESPACE` e `--runs-dir` do comando de migração.
- `shell_coder_sandbox`, `chess_engine_build` e `trend_blog` preparam automaticamente o bundle compartilhado. O preparador de Git foi extraído do engineering ticket e reutilizado, preservando revisão fixada, ausência de remotes, timeout e cleanup por `TempDir`.
- O bundle pequeno é materializado em diretórios temporários independentes por leitura. O shell/coder lê os assets verificados em memória e descarta a origem temporária; avaliação e captura materializam novamente o bundle incorporado, independentemente do workspace alterado pelo candidato. Não há cache persistente ou estado adicional no contexto nesta etapa.
- O launcher exact-stack deixou de preparar e repassar `HARNESS_E2E_FIXTURE_PATH`. O override existente do engineering ticket no executor protegido permanece com seu contrato de posse e cleanup.
- A prontidão dos três cenários verifica Git sem preparar checkouts. O setup existente valida o conteúdo e prepara o workspace antes de chamar o modelo. Os requisitos dos demais cenários ainda seguem o comportamento anterior.
- A remoção do campo obsoleto `fixture_path_env` altera o contrato materializado de `chess_engine_build`, agora na versão 4. Planos salvos com essa versão anterior precisam ser editados e salvos para materializar o contrato atual antes de outra execução. Resultados históricos não são reescritos.

Permanecem para os próximos incrementos: derivar todos os diretórios de scratch de uma configuração comum, eliminar os overrides opcionais de workspace, instalação portátil do banco via Compose, Kanban, incidente, segurança, isolamento SWE e proveniência da stack. Este incremento não instala dependências de sistema nem reinicia a stack.

Validação local: `cargo test --locked --all-targets` passou com 719 testes e 4 ignorados; `cargo clippy --locked --all-targets -- -D warnings` passou. A suíte Python passou com 265 testes, incluindo a execução da preparação do grupo compartilhado sem checkout ou launcher externo. Os testes das três fixtures também passaram a partir de um diretório vazio, com todas as variáveis `HARNESS_E2E_*` removidas do ambiente. Foram verificados baseline, manifests, independência das cópias e cleanup. Nenhuma execução completa com modelo ou instalação publicada em máquina limpa foi realizada.

## Objetivo

Adicionar o E2E à stack iii e executar os cenários suportados sem preencher variáveis de fixture, preparar clones, produzir arquivos de runtime ou descobrir caminhos de banco. O Compose continua sendo a entrada; o E2E passa a preparar os recursos de cada tentativa.

A meta inicial considera uma stack iii com Harness e acesso aos modelos configurados, em uma plataforma de execução suportada. Credenciais dos provedores, seleção do teste/modelo e autorização para executar continuam sendo decisões do usuário. Instalar Docker ou permissões de isolamento em qualquer máquina é uma responsabilidade adicional da distribuição do executor, não algo resolvido apagando variáveis.

## Diagnóstico inicial

O levantamento encontrou **66 nomes `HARNESS_E2E_*` referenciados** nos arquivos versionados de implementação, scripts e workflows examinados. Isso inclui build, CI, testes, opções com default e variáveis apenas repassadas. **Não são 66 entradas obrigatórias para iniciar o worker.** O inventário completo e o método estão no fim deste documento.

Há quatro configurações de recursos que bloqueiam cenários específicos:

| Configuração atual | Consumidores e comportamento observado | Mudança proposta |
| --- | --- | --- |
| `HARNESS_E2E_FIXTURE_PATH` | `shell_coder_sandbox`, `chess_engine_build` e `trend_blog` exigem um checkout descartável. O launcher oficial já prepara o bundle compartilhado. | Materializar o bundle revisado em um clone privado da tentativa, dentro do próprio E2E. |
| `HARNESS_E2E_KANBAN_RUNTIME` | Kanban exige JSON com fixture, imagem, Node, iii, pnpm, dependências e navegador. O bootstrap depende do checkout do projeto e instala arquivos nele. | Distribuir os assets necessários e preparar o runtime automaticamente, com versões fixadas e cache imutável. Cada tentativa recebe seu checkout gravável. |
| `HARNESS_E2E_INCIDENT_FIXTURE_PATH` | Incidente exige um clone canônico com contrato e referências `known_good`/`incident`, já ao construir o runtime. Não foi encontrado um bundle canônico distribuível ou provisionador nos arquivos versionados examinados. | Primeiro versionar uma fixture revisada que cumpra o contrato; depois materializá-la antes da construção do workflow. |
| `HARNESS_E2E_SECURITY_FIXTURE_PATH` | Segurança exige o clone no preflight mesmo quando as funções do worker externo existem. A presença da variável também permite registrar o adapter local quando essas funções estão ausentes. | Preparar o clone separadamente da seleção explícita do backend. Registrar se o sujeito é o worker externo ou o adapter local; nunca trocar um pelo outro silenciosamente. |

Evidências: [fixture compartilhada](../src/scenarios/shell_coder_sandbox.rs), [launcher oficial](../scripts/run_exact_stack_group.sh), [setup Kanban](../src/scenarios/kanban/mod.rs), [bootstrap Kanban](../scripts/kanban_eval/bootstrap.py), [incidente](../src/workflow/incident_response/helpers.rs), [preflight de segurança](../src/workflow/security_scan/operations.rs), [registro do adapter](../src/workflow/security_scan/local_adapter.rs).

No modo de integração externa, segurança também depende de estado remoto: o [template versionado](../tests/fixtures/security-scan-repository/README.md) exige alertas Dependabot e code scanning preparados no repositório privado. Automatizar o clone não prepara esses serviços. O executor oficial deve possuir uma fixture remota controlada e verificar seu acesso/estado antes da execução; esse provisionamento ainda precisa ser definido. A indisponibilidade dessa integração deve bloquear o modo externo, sem ativar o adapter local.

O projeto já tem uma implementação adequada para reutilizar: [engineering_ticket/fixture.rs](../src/scenarios/engineering_ticket/fixture.rs) incorpora o bundle, cria um diretório temporário, materializa a revisão fixada e mantém a posse do diretório para cleanup. `HARNESS_E2E_ENGINEERING_TICKET_FIXTURE_PATH` é um override opcional; esse cenário já funciona sem um caminho fornecido pelo usuário. SWE também incorpora seus assets e deriva o diretório de trabalho quando o override está ausente.

### Inicialização e persistência têm duas fontes de configuração

O worker só exige `III_URL`, `III_NAMESPACE`, `III_WORKER_NAME` e `III_CONFIG`, fornecidos pelo Compose. Seu YAML define `data_dir`, `control_database` e `control_namespace`. Modelo, juiz e caminhos de fixtures não são necessários para subir a interface. [Fonte](../src/worker.rs).

O setup documentado exige iniciar dois arquivos Compose em ordem. [worker-compose.control.yaml](../worker-compose.control.yaml) depende de `path://../workers/database` e contém um caminho absoluto da máquina do autor; [worker-compose.yaml](../worker-compose.yaml) aponta para o banco desse namespace. O pacote declara dependências, mas isso não comprova que a instalação inicial configure automaticamente o banco nomeado e a ligação entre namespaces.

Além disso, `migrate-storage` cria a persistência por `Persistence::from_client`, que lê `HARNESS_E2E_CONTROL_DATABASE` e `HARNESS_E2E_CONTROL_NAMESPACE`, enquanto o worker usa o YAML. Uma configuração personalizada pode fazer o comando administrativo procurar outro banco. [CLI](../src/main.rs), [persistência](../src/persistence.rs).

Proposta: uma configuração efetiva do projeto deve determinar banco, namespace proprietário e diretórios tanto no worker quanto nos comandos administrativos. Preservar a separação entre o namespace do sujeito avaliado e o dono do banco de controle; derivar valores coerentes não significa igualá-los. Remover caminhos pessoais e dependência obrigatória do checkout de Workers no fluxo publicado.

### A prontidão do plano não cobre a preparação real

`PlanStore::requirements` valida configuração, revisão, executor e contratos. Entretanto, adiciona os requisitos de fixtures com estado `pending` e calcula `ready` pela ausência de `blocked`. Assim, um plano pode ser admitido sem ter demonstrado que seus recursos estão disponíveis. [Fonte](../src/plans/store.rs).

Proposta: aproveitar essa mesma verificação e a preparação existente dos cenários. Mostrar `preparando`, `pronto` ou `bloqueado`, com o requisito concreto. Antes de chamar o modelo, verificar o isolamento efetivo e preparar somente os recursos do escopo selecionado. O plano pode ser salvo durante a preparação; a execução só avança quando seus requisitos estiverem prontos. A preparação não precisa invocar modelo.

### Algumas opções não resolvem o que o nome sugere

- SWE repassa `HARNESS_E2E_SWE_ISOLATION_BACKEND` e `HARNESS_E2E_SWE_DOCKER_IMAGE`, mas o isolador Python não os consulta. `DOCKER_HOST` também é repassado até o controlador, porém os subprocessos Docker do isolador usam um ambiente fixo que o exclui. Remover esse repasse ineficaz no caminho SWE; esta conclusão não se estende a outros usos de Docker. [Assets](../src/scenarios/swe_service/assets.rs), [isolador](../src/scenarios/swe_service/isolation.py).
- O isolador SWE tenta Bubblewrap e depois imagens oficiais Python já presentes no Docker. Uma máquina com Docker, mas sem uma imagem utilizável em cache, continua bloqueada. Preparar uma imagem fixada no executor suportado elimina esse passo manual; executar código do candidato diretamente no host não é uma alternativa equivalente.
- `HARNESS_E2E_RUN_DIR`, `HARNESS_E2E_RUNS_DIR`, `HARNESS_E2E_OUTPUT` e `data_dir` atendem camadas diferentes. Derivar os diretórios de uma configuração do projeto, mantendo scratch separado de evidências retidas e permitindo destino explícito de exportação na CLI.
- `HARNESS_E2E_REGISTRY_IMPLEMENTATION` é uma entrada de experimento para verificação avulsa. No fluxo ordenado implementação → verificação, o contexto já compartilha a entrega. Substituir o caminho externo por referência explícita à entrega no plano; nunca selecionar arbitrariamente a última execução. [Fonte](../src/scenarios/registry.rs).

### Identidade deve continuar verificável

No modo `source`, a ausência de `HARNESS_E2E_WORKERS_REPOSITORY`/`REVISION` usa a identidade de build do próprio E2E como default. Isso não comprova qual revisão de Workers/Harness está sendo avaliada. [Fonte](../src/identity.rs).

A retirada dessas entradas do setup deve vir acompanhada da identidade efetiva obtida do artefato instalado ou contrato da stack. Manter versão/digest de runtime e fixture na evidência. O launcher exact-stack já deriva parte dessas informações de contratos; reutilizar essa resolução sem inventar identidade quando ela não estiver disponível. [Geração da campanha](../scripts/exact_stack_campaign.py), [resolução da stack](../scripts/resolve_stack_lock.py).

## Fluxo proposto

1. **Adicionar o worker pelo Compose.** Resolver a dependência publicada de banco e aplicar a configuração de controle e de diretórios do projeto. A Console abre sem formulário de configuração de fixtures.
2. **Selecionar ou abrir um plano.** Usar o catálogo de modelos registrado e a escolha salva no plano. Havendo um default explicitamente configurado no Harness, ele pode preencher a seleção; não escolher um modelo arbitrário.
3. **Preparar o escopo selecionado.** Validar ferramentas e isolamento; materializar bundles; obter assets fixados quando faltarem; criar os diretórios privados da tentativa. Exibir progresso na prontidão que já existe.
4. **Executar com recursos e identidade explícitos.** Passar os caminhos pelo contexto da tentativa. Não alterar `std::env` global para transportar configuração entre execuções. Preparação de incidente e segurança deve ocorrer antes dos respectivos preflights e da construção que já consulta a fixture.
5. **Encerrar e preservar evidências.** Capturar os artefatos necessários antes do cleanup, remover processos e clones pertencentes à tentativa e conservar evidências segundo a política de retenção. Cache compartilhado contém apenas assets imutáveis verificados; sua publicação precisa tolerar preparações concorrentes.

Esse fluxo reutiliza `WorkerConfig`, a prontidão do plano, `E2eContext` e os ciclos de setup/cleanup. Não exige um novo serviço de provisionamento, framework de plugins ou comando principal paralelo ao Compose.

O suporte atual do iii para instalar/configurar automaticamente dependências em namespaces distintos ainda precisa ser validado ponta a ponta. A leitura do Compose mostra `config_override` e `start_after`, mas não comprova uma declaração completa desse fluxo. Se houver lacuna, a integração mínima deve ser tratada no iii/Workers; não apresentar um comando novo como se já existisse.

## Sequência de implementação sugerida

| Etapa | Mudança delimitada | Critério de conclusão |
| --- | --- | --- |
| 1. Configuração e prontidão | Unificar resolução de configuração; tornar o Compose portátil; verificar os requisitos reais do plano. Validar o contrato de instalação com iii/Workers. | Novo projeto sobe sem paths pessoais; migração usa o mesmo banco do worker; recurso ausente bloqueia antes do modelo. |
| 2. Fixtures pequenas | Usar o padrão do engineering ticket para a fixture compartilhada; distribuir o incidente revisado; automatizar clones de segurança com backend explícito. | Remover a necessidade das três variáveis de checkout sem alterar o significado dos testes. |
| 3. Runtime de cenários | Distribuir/preparar Kanban e a imagem SWE fixada na plataforma suportada. Reutilizar assets existentes e remover o repasse de opções SWE sem efeito. | Nenhum arquivo de runtime preparado pelo usuário; primeira execução prepara recursos e as seguintes reutilizam o cache verificado. |
| 4. Distribuição e limpeza | Validar o pacote publicado sem checkout; adequar o launcher exact-stack ao mesmo contrato; remover superfícies obsoletas e atualizar a documentação. | Mesmo cenário e mesma identidade reproduzíveis localmente e no executor oficial, respeitando os perfis protegidos. |

As mudanças internas de preparação e UI pertencem ao E2E. Eventuais mudanças no contrato do Compose/instalação ou no despacho do Release Control devem continuar em branches de integração, conforme a separação já adotada no projeto. Não há evidência suficiente nesta auditoria para afirmar que RC precisa mudar.

### Aceitação

- Instalação publicada em uma plataforma suportada, com diretório de usuário limpo e sem checkout irmão de Workers.
- Nenhuma variável `HARNESS_E2E_*` preenchida manualmente no fluxo normal de instalação e execução de planos suportados. Variáveis internas de CI/build podem continuar existindo.
- Fixtures e runtime derivados do plano, com revisão/digest registrados; adapter local e worker externo identificados separadamente.
- Cache aquecido permite repetir a preparação sem rede; acessos de rede inerentes ao cenário continuam sujeitos ao seu contrato.
- Tentativas em execuções independentes não compartilham checkout gravável; cancelamento libera processos e recursos temporários sem apagar transcripts e artefatos retidos.
- Falta de provider, ferramenta ou isolamento aparece como impedimento de infraestrutura antes da chamada ao modelo.
- Cenários que exigem supervisor protegido continuam no executor apropriado; a instalação local não amplia sua admissibilidade.
- Validação positiva e negativa das fixtures demonstra que a automação preservou o contrato de avaliação.

## Inventário completo

Método: busca lexical de `HARNESS_E2E_[A-Z0-9_]+` nos arquivos versionados `.rs`, `.py`, `.mjs`, `.sh`, `.yaml` e `.yml` de `src/`, `scripts/`, `.github/`, mais `build.rs`, `iii.worker.yaml` e os dois arquivos Compose. Foram encontrados 62 nomes completos e o prefixo dinâmico `HARNESS_E2E_STORAGE_`; expandir as quatro classes de retenção produz os 66 nomes abaixo. Uma referência pode ser definição, leitura, emissão ou repasse, não necessariamente uma opção funcional. Arquivos locais ignorados, valores de segredos e configurações efetivas de processos não fazem parte da contagem.

Na tabela, todos os nomes têm o prefixo **`HARNESS_E2E_`**. O destino é uma proposta, não comportamento já implementado.

| Sufixo | Uso atual | Destino proposto |
| --- | --- | --- |
| `ADMISSION_TIMEOUT_SECONDS` | Prazo de admissão no launcher exact-stack | Interno do executor |
| `ARTIFACTS_DIR` | Destino dos artefatos da campanha/CI | Derivado da execução |
| `AUDIT_MODEL` | Modelo de auditoria opcional na CLI | Configuração explícita da execução |
| `AUDIT_PROVIDER` | Provider de auditoria opcional na CLI | Configuração explícita da execução |
| `BIN` | Binário escolhido pelo runner de campanhas | Interno de desenvolvimento/CI |
| `BUILD_REPOSITORY` | Proveniência incorporada no build | Manter no build |
| `BUILD_REVISION` | Proveniência incorporada no build | Manter no build |
| `CAMPAIGN_GROUP` | Grupo selecionado pelo runner | Derivado do plano/campanha |
| `CAMPAIGN_GROUP_ID` | Identificador de grupo no despacho | Derivado do plano/campanha |
| `CAMPAIGN_ID` | Identificador emitido pelo runner | Derivado da execução |
| `CAMPAIGN_OUTPUT` | Saída de campanha | Derivada da execução/exportação |
| `COMPOSE_ADD_TIMEOUT_SECONDS` | Prazo do launcher para adicionar workers | Interno do executor |
| `CONTROL_DATABASE` | Banco usado pelo construtor de persistência da CLI | Configuração efetiva comum com o worker |
| `CONTROL_NAMESPACE` | Namespace do banco usado pelo construtor da CLI | Configuração efetiva comum com o worker |
| `DURABLE_TIMEOUT_MS` | Timeout do arquivo durável; default 120000 ms | Default/configuração operacional |
| `ENGINEERING_FIXTURE_REPOSITORY` | Origem escolhida pelo provisionador de CI | Asset revisado do cenário |
| `ENGINEERING_FIXTURE_ROOT` | Diretório do provisionador de CI | Derivado da tentativa |
| `ENGINEERING_TICKET_FIXTURE_PATH` | Override opcional; bundle automático já existe | Preparação nativa, sem caminho manual |
| `ENGINE_PORT` | Porta escolhida pelo launcher | Interno do executor |
| `ENGINE_REVISION` | Revisão opcional de proveniência | Identidade efetiva da stack |
| `FAULT_SUPERVISOR` | Programa supervisor de falhas | Interno do executor protegido |
| `FIXTURE_LAUNCHER` | Caminho do preparador de fixtures no checkout | Preparação do cenário distribuída com o executor |
| `FIXTURE_PATH` | Checkout obrigatório dos três cenários compartilhados | Clone privado preparado automaticamente |
| `FIXTURE_SOURCE_ROOT` | Raiz dos assets usados pelo launcher | Assets distribuídos com o executor |
| `HARNESS_ROOT` | Checkout do runner usado pelos scripts oficiais | Interno de build/CI; pacote independente de checkout |
| `HISTORY_DATABASE` | Histórico durável; default `primary` | Configuração operacional coerente com o serviço de histórico |
| `INCIDENT_FIXTURE_PATH` | Clone obrigatório do incidente | Fixture revisada e clone automático |
| `JUDGE_MODEL` | Juiz para cenários que o exigem; também default do formulário de execução rápida | Configuração explícita do plano/execução |
| `JUDGE_PROVIDER` | Provider do juiz | Configuração explícita do plano/execução |
| `KANBAN_BOOTSTRAP` | Script de preparação usado pelo launcher | Preparação distribuída com o executor |
| `KANBAN_FIXTURE_ROOT` | Checkout Kanban usado pelo launcher | Asset fixado e clone privado |
| `KANBAN_RUNTIME` | JSON obrigatório do runtime Kanban | Contexto preparado automaticamente |
| `LANE` | Classificação operacional da execução | Derivada do despacho |
| `MODEL` | Modelo da CLI/campanha; default do formulário de execução rápida | Seleção no plano/execução |
| `OUTPUT` | Saída CLI; defaults `target/e2e` ou `target/e2e-replay` | Derivada do projeto ou destino explícito de exportação |
| `PROGRESS_INTERVAL_SECONDS` | Intervalo de progresso CLI; default 15 s | Opção interna/CLI |
| `PROVIDER` | Provider da CLI/campanha; default do formulário de execução rápida | Seleção no plano/execução |
| `REGISTRY_API_URL` | Endpoint usado na resolução da stack | Configuração operacional do Registry |
| `REGISTRY_IMPLEMENTATION` | Entrega externa para verificação Registry avulsa | Referência explícita à entrega no plano |
| `REPOSITORY` | Override da identidade do E2E | Proveniência real do artefato |
| `REVISION` | Override da revisão do E2E | Proveniência real do artefato |
| `RUNS_DIR` | Diretório de histórico usado por comandos CLI | Configuração efetiva do projeto |
| `RUN_DIR` | Raiz temporária dos cenários; default do sistema | Derivada da tentativa, separada de evidências |
| `RUN_TIMEOUT_SECONDS` | Prazo de execução no launcher | Política do executor |
| `SECRET_ENV_NAMES` | Lista adicional de nomes a redigir dos artefatos | Política operacional de redação |
| `SECURITY_FIXTURE_PATH` | Clone obrigatório e habilitação condicional do adapter local | Clone automático; backend explícito e independente |
| `SEED` | Semente da execução | Configuração do plano |
| `STACK_DIGEST` | Digest da stack resolvida | Derivado do contrato da stack |
| `STACK_LOCK` | Arquivo de lock consumido pelo launcher | Interno do despacho/reprodução |
| `STACK_MODE` | Proveniência `source` ou `registry` | Derivada da origem efetiva |
| `STACK_VERSIONS` | Versões exatas da stack Registry | Derivadas da resolução da stack |
| `STORAGE_BACKUP_BUCKET` | Bucket de backup; default `e2e-canonical` | Configuração operacional de armazenamento |
| `STORAGE_TEMPORARY_BUCKET` | Retenção temporária; default `e2e-temporary` | Configuração operacional de armazenamento |
| `STORAGE_PULL_REQUEST_BUCKET` | Retenção de PR; default `e2e-pull-request` | Configuração operacional de armazenamento |
| `STORAGE_LONGITUDINAL_BUCKET` | Retenção longitudinal; default `e2e-longitudinal` | Configuração operacional de armazenamento |
| `STORAGE_CANONICAL_BUCKET` | Retenção canônica; default `e2e-canonical` | Configuração operacional de armazenamento |
| `SUITE_DEADLINE_SECONDS` | Limite global da suíte | Política do executor |
| `SWE_DOCKER_IMAGE` | Repassada, sem consumo pelo isolador SWE | Remover repasse ineficaz; imagem fixada no runtime |
| `SWE_ISOLATION_BACKEND` | Repassada, sem consumo pelo isolador SWE | Remover repasse ineficaz; verificar isolamento efetivo |
| `SWE_WORKSPACE_ROOT` | Override opcional da raiz SWE | Derivada da tentativa |
| `TECHNICAL_RETRIES` | Quantidade de retries técnicos | Configuração do plano/execução |
| `TEST_DATABASE_URL` | Banco para testes automatizados do projeto | Exclusivo de testes/CI |
| `UPDATE_SCHEMAS` | Atualização dos schemas em testes | Exclusivo de desenvolvimento |
| `WAIT_SECONDS` | Prazo de prontidão no launcher | Interno do executor |
| `WORKERS_REPOSITORY` | Identidade da origem Workers avaliada | Identidade efetiva do sujeito |
| `WORKERS_REVISION` | Revisão de Workers avaliada | Identidade efetiva do sujeito |

Fontes principais por grupo: [CLI](../src/main.rs), [defaults da execução rápida](../src/dashboard/controller.rs), [proveniência](../src/identity.rs), [persistência](../src/persistence.rs), [arquivo durável](../src/durable.rs), [redação](../src/redaction.rs), [launcher](../scripts/run_exact_stack_group.sh), [campanhas](../scripts/run_e2e_campaign.py), [geração da stack](../scripts/exact_stack_campaign.py), [workflow oficial](../.github/workflows/exact-stack-e2e.yml).

### Variáveis fora do prefixo

| Grupo | Avaliação |
| --- | --- |
| `III_URL`, `III_NAMESPACE`, `III_WORKER_NAME`, `III_CONFIG` | Contrato de inicialização injetado pelo Compose. Manter; não são perguntas do onboarding. |
| `OPENAI_API_KEY`, `GITHUB_TOKEN`, `GH_TOKEN`, `AWS_SECRET_ACCESS_KEY`, `CLOUDFLARE_API_TOKEN` em `redaction.rs` | Leitura para ocultar valores sensíveis. Essa ocorrência não os torna credenciais obrigatórias do E2E. Credenciais dos workers e do CI seguem seus próprios contratos. |
| `TARGET`, `CARGO_MANIFEST_DIR`, `SKIP_CONSOLE_UI_BUILD`, `PNPM` | Build a partir do código. O binário publicado incorpora os assets da Console; essas opções não devem aparecer na instalação de usuário. |
| `UPDATE_HISTORY_SCHEMA`, `HISTORY_EXPORT_ARTIFACT`, `SWE_FIXTURE_ROOT`, `SWE_REQUIRE_OS_ISOLATION`, `SWE_REQUIRE_DOCKER_ISOLATION`, `CSS_DEBT_UPDATE` | Desenvolvimento e validação do próprio projeto. Manter fora do onboarding. |
| `HOME`, `PATH`, `TMPDIR`, `RUST_LOG` e ambiente do sistema | Convenções de runtime e diagnóstico. A meta elimina configuração manual do E2E, não o uso de ambiente pelo sistema operacional. |

## Limites da auditoria

Na auditoria inicial foram examinados os consumidores no código, os manifests, os provisionadores e os workflows, sem executar cenários ou chamadas a modelo. O primeiro incremento foi implementado depois dessa análise, conforme descrito no início. A promessa de instalação limpa, a instalação automática entre namespaces e a distribuição completa de Kanban ainda exigem as validações descritas acima. As mudanças locais anteriores e a stack em execução foram preservadas.
