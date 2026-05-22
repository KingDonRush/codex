# Plano: Codex com Claude nativo

Data base: 2026-05-20.

Este arquivo e o artefato principal do trabalho agora. Ele nao e uma promessa de
implementacao pronta. Ele e o plano que deve impedir alucinacao, drift e decisoes
orais que somem quando outra pessoa ou outro agente assumir.

## Mantra do plano

Como este projeto continua funcionando depois que o usuario parar de explicar?

Resposta operacional:

- toda decisao precisa apontar para evidencia no repo, documentacao oficial ou
  uma lacuna marcada como "a provar";
- nenhum comportamento essencial do Codex pode depender de memoria de conversa;
- cada fase precisa ter criterio de aceite, arquivos provaveis de mudanca,
  testes esperados e criterio de rollback;
- toda diferenca entre OpenAI Responses e Anthropic Messages precisa ser
  convertida numa fronteira explicita, nao espalhada pela UI, sandbox ou loop de
  ferramentas.

## Objetivo

Manter o harness do Codex e trocar o backend de modelo para Claude/Anthropic.

Harness que deve continuar vivo:

- CLI/TUI/app-server que o Desktop usa;
- sandbox e politicas de permissao;
- execucao de comandos;
- `apply_patch`;
- MCP local;
- ferramentas dinamicas, plugins, skills e subagentes;
- historico de conversa e retomada depois de tool calls;
- streaming, token usage, erros e cancelamento;
- compaction ou estrategia equivalente para contexto longo.

Nao objetivo:

- "enganar" o Codex via LiteLLM ate parecer OpenAI Responses;
- colocar chave API real em arquivo;
- reescrever UI antes de provar o provider;
- assumir que Claude gera imagem nativamente como a Responses API;
- assumir que MCP remoto da Anthropic substitui MCP local do Codex.

## Evidencia base

Repo inspecionado: `codex-claude-fork/openai-codex-upstream`.

Fontes locais:

- `AGENTS.md`: regras de manutencao. Evitar crescer `codex-core`; novas ideias
  devem preferir crates/modulos proprios. Mudanca em config exige
  `just write-config-schema`. Rust exige `just fmt` e testes por crate.
- `codex-rs/core/README.md`: `codex-core` contem logica de negocio e assume
  sandbox helpers e simulacao de `apply_patch`. O provider nao deve mexer nesses
  contratos.
- `codex-rs/codex-api/README.md`: `codex-api` e a camada wire-level. Ela ja
  possui requests, headers, retry, SSE e conversao para `ResponseEvent`.
- `codex-rs/tools/README.md`: `codex-tools` e a crate certa para modelos e
  adaptadores de ferramentas compartilhaveis. Nao deve virar pasta generica.
- `codex-rs/protocol/README.md`: `codex-protocol` deve ficar com tipos e pouca
  logica de negocio.
- `codex-rs/model-provider-info/src/lib.rs`: `WireApi` hoje aceita apenas
  `Responses`. `wire_api = "chat"` foi removido explicitamente.
- `codex-rs/model-provider/src/provider.rs`: `ProviderCapabilities` ja modela
  `namespace_tools`, `image_generation` e `web_search`.
- `codex-rs/model-provider/src/amazon_bedrock/mod.rs`: exemplo de provider que
  desliga capacidades nao suportadas.
- `codex-rs/core/src/client.rs`: `ModelClient::stream` hoje faz `match` apenas
  em `WireApi::Responses`.
- `codex-rs/core/src/tools/spec_plan.rs`: monta ferramentas visiveis ao modelo,
  usa `provider.capabilities()` para namespace/web/image e controla deferred
  tools/tool_search.
- `codex-rs/tools/src/tool_spec.rs`: `ToolSpec` e serializado no formato OpenAI
  Responses, incluindo `namespace`, `web_search`, `image_generation` e
  `tool_search`.
- `codex-rs/protocol/src/models.rs`: `ResponseItem::FunctionCall` ja preserva
  `namespace`, `name`, `arguments` e `call_id`.
- `codex-rs/core/src/tools/router.rs`: o dispatcher ja reconstrui `ToolName`
  com `ToolName::new(namespace, name)`. Isso e a fronteira que deve continuar.
- `codex-rs/protocol/src/tool_name.rs`: `ToolName` tem `name` e `namespace`
  separados, mas `Display` concatena namespace+name. Nao usar `Display` como
  formato reversivel.

Fontes oficiais externas usadas para limitar suposicoes:

- Anthropic Messages API e stateless: o cliente envia o historico inteiro.
  https://platform.claude.com/docs/en/build-with-claude/working-with-messages
- Anthropic streaming usa `message_start`, `content_block_start`,
  `content_block_delta`, `content_block_stop`, `message_delta` e `message_stop`.
  Tool input chega como `input_json_delta` parcial.
  https://platform.claude.com/docs/en/build-with-claude/streaming
- Ferramentas Anthropic podem ser client-side ou server-side. Client tools
  retornam `tool_use` e o cliente devolve `tool_result`.
  https://platform.claude.com/docs/en/agents-and-tools/tool-use/overview
- Nomes de ferramentas Anthropic devem casar com
  `^[a-zA-Z0-9_-]{1,64}$`.
  https://platform.claude.com/docs/en/agents-and-tools/tool-use/define-tools
- MCP connector da Anthropic existe, mas e remoto HTTP/SSE, beta, e nao substitui
  automaticamente MCP STDIO/local do Codex.
  https://platform.claude.com/docs/en/agents-and-tools/mcp-connector

## Diagnostico

O problema nao esta no sandbox, no shell, no apply_patch nem no runtime de MCP.
O problema esta na fronteira wire entre Codex e o modelo.

O experimento via LiteLLM mostrou:

- chamada simples com function tool pode funcionar;
- ferramenta OpenAI `type = "namespace"` quebra em provider que nao entende
  Responses API;
- `web_search`, `image_generation` e `tool_search` tambem sao formas hosted ou
  especiais do Responses API;
- por isso, gateway OpenAI-compatible nao e suficiente para "tudo que o Codex
  faz".

Conclusao: o plano sustentavel e provider nativo Anthropic Messages, com
adaptadores claros de request, stream, ferramentas e historico.

## Contratos que nao podem quebrar

1. A UI e o loop do Codex devem continuar consumindo `ResponseEvent`.
2. O dispatcher deve continuar recebendo `ResponseItem::FunctionCall`.
3. O runtime de ferramenta deve continuar recebendo `ToolName { namespace, name
   }`, nao nomes achatados de provider.
4. `call_id` precisa sobreviver ida e volta, pois liga `tool_use` a
   `tool_result`.
5. `apply_patch`, shell, MCP, plugins e subagentes devem continuar passando pelo
   registry/router do Codex.
6. Chave Anthropic ou gateway nao entra em arquivo versionado. Usar env/header.
7. Provider capabilities nao podem anunciar recurso antes de ter teste de
   fidelidade.
8. Se uma feature OpenAI nao existir em Claude, ela vira adaptador separado ou
   fica explicitamente desabilitada ate existir prova.

## Arquitetura alvo

Fronteira interna que deve permanecer:

```text
TurnContext/Prompt
  -> ToolRouter.model_visible_specs()
  -> provider request adapter
  -> provider stream parser
  -> ResponseEvent
  -> Session/ToolRouter dispatcher
```

Mudancas planejadas:

1. `codex-rs/model-provider-info`
   - adicionar `WireApi::AnthropicMessages`;
   - aceitar `wire_api = "anthropic_messages"` no TOML;
   - criar helper de provider Anthropic, se for builtin;
   - manter `wire_api = "chat"` morto.

2. `codex-rs/model-provider`
   - criar runtime provider Anthropic ou uma configuracao detectavel;
   - capabilities iniciais conservadoras:
     - fase texto/function tools: `namespace_tools=false`,
       `image_generation=false`, `web_search=false`;
     - depois do flatten/unflatten testado: `namespace_tools=true`;
     - `web_search=true` apenas quando o adaptador Anthropic server-tool estiver
       mapeado e testado;
     - `image_generation=true` apenas se houver ferramenta/provider de imagem
       separado e testado.
   - nao usar `env_key` para Anthropic se ele gerar `Authorization: Bearer`.
     A rota minima e `env_http_headers = {"x-api-key": "ANTHROPIC_API_KEY"}`
     mais `http_headers = {"anthropic-version": "2023-06-01"}`. Alternativa
     mais limpa: introduzir esquema de auth por provider, mas isso aumenta
     superficie de config.

3. `codex-rs/codex-api`
   - adicionar modulo Anthropic Messages no nivel wire:
     - request structs Anthropic;
     - conversor de `ResponseItem`/historico;
     - conversor de ferramentas;
     - mapa reversivel de nomes Anthropic-safe para `ToolName`;
     - endpoint `/messages`;
     - parser SSE Anthropic para `ResponseEvent`.
   - manter `codex-core` com uma mudanca pequena: no `match WireApi`, delegar
     para o client Anthropic.

4. `codex-rs/tools`
   - manter `ToolSpec` global no formato interno/Responses;
   - nao mover logica wire Anthropic para ca sem necessidade;
   - considerar extrair adaptador compartilhado somente se outro crate precisar
     do mesmo contrato.

5. `codex-rs/protocol`
   - evitar logica Anthropic aqui;
   - so mudar se uma extensao de tipo for inevitavel e provada por teste.

## Estrategia de request Anthropic

Anthropic Messages exige um formato diferente do Responses API. O adaptador deve
ser uma camada de traducao, nao uma contaminacao do resto do Codex.

Mapeamento de entrada:

- `prompt.base_instructions.text` vira `system`;
- `ResponseItem::Message` vira mensagens Anthropic com role equivalente;
- `ContentItem::InputText` e `OutputText` viram blocos `text`;
- `ContentItem::InputImage` precisa seguir formato Anthropic de imagem; isto e
  um gate proprio porque Codex usa `image_url`;
- `ResponseItem::FunctionCall` anterior vira bloco assistant `tool_use`;
- `ResponseItem::FunctionCallOutput` vira mensagem user com bloco
  `tool_result`;
- `ResponseItem::ToolSearchOutput`, `CustomToolCallOutput` e outputs de MCP com
  conteudo estruturado precisam de fixture antes de liberar;
- `ResponseItem::Reasoning` OpenAI encrypted content nao deve ser enviado para
  Anthropic sem decisao explicita.

Parametros obrigatorios Anthropic a definir:

- `model`: vem de `ModelInfo.slug`;
- `max_tokens`: Anthropic exige; vem de `ModelInfo.max_output_tokens`, carregado
  pelo catalogo/provider. O adapter Anthropic falha se esse campo estiver
  ausente, em vez de chutar valor dentro do client;
- `stream: true`;
- `tools`: apenas depois de conversao por provider;
- `tool_choice`: mapear `auto` quando equivalente; outras escolhas so com
  teste.

## Estrategia de stream Anthropic

O parser Anthropic deve emitir os mesmos `ResponseEvent` que o resto do Codex ja
entende.

Mapeamento esperado:

- `message_start`:
  - guardar `message.id` como `response_id`;
  - emitir `ResponseEvent::Created`;
  - se vier modelo efetivo, emitir `ServerModel`.
- `content_block_start` com `type=text`:
  - iniciar buffer de texto por index.
- `content_block_delta` com `text_delta`:
  - emitir `ResponseEvent::OutputTextDelta(delta)`.
- `content_block_start` com `type=tool_use`:
  - guardar `id`, `name` e index;
  - resolver `name` pelo `ToolNameMap` quando possivel.
- `content_block_delta` com `input_json_delta`:
  - acumular JSON parcial;
  - opcionalmente emitir `ToolCallInputDelta` se a UI precisar ver streaming de
    argumentos.
- `content_block_stop` de tool:
  - parsear JSON acumulado;
  - emitir `ResponseEvent::OutputItemDone(ResponseItem::FunctionCall { ... })`
    com namespace/name internos restaurados.
- `message_delta`:
  - capturar `stop_reason` e usage;
  - `tool_use` nao deve ser tratado como erro, e sim como turno que precisa
    dispatch de ferramenta.
- `message_stop`:
  - emitir `ResponseEvent::Completed { response_id, token_usage, end_turn }`;
  - `end_turn` deve ser `Some(true)` para `end_turn`, `Some(false)` para
    `tool_use`, e `None` quando a semantica nao for segura.
- `error`:
  - mapear overloaded/rate-limit/context para `ApiError` equivalente quando
    possivel;
  - desconhecidos viram erro stream com payload resumido.

Gates:

- fixture de stream texto;
- fixture de stream tool_use com input JSON fragmentado;
- fixture de erro;
- fixture de tool_use + tool_result + segunda resposta.

## Estrategia de ferramentas

### Function tools simples

`ToolSpec::Function(ResponsesApiTool)` deve virar ferramenta Anthropic:

- `name`: nome Claude-safe;
- `description`: descricao original;
- `input_schema`: `parameters`;
- `strict`: preservar quando compativel;
- `output_schema`: inicialmente ignorar ou mapear apenas depois de prova.

Aceite:

- `exec_command` aparece para Claude;
- Claude chama `exec_command`;
- Codex despacha pelo mesmo handler;
- output volta como `tool_result`;
- segunda chamada do modelo continua o turno.

### Namespaces e MCP

O Codex usa `ToolSpec::Namespace`, mas Anthropic client tools nao aceitam esse
tipo OpenAI. Ao mesmo tempo, Anthropic exige nome de ferramenta com regex
`^[a-zA-Z0-9_-]{1,64}$`. Portanto o plano nao pode depender de concatenar
`namespace + name` de forma solta.

Design obrigatorio:

- criar `ToolNameMap` por request;
- cada ferramenta interna tem uma chave `ToolName { namespace, name }`;
- gerar um nome Claude-safe:
  - function tool sem namespace usa o nome normalizado;
  - namespace usa `namespace + name` quando o namespace ja termina em `_`, ou
    `namespace + "_" + name` nos demais casos;
  - caracteres fora de `[a-zA-Z0-9_-]` viram `_`;
  - nomes acima de 64 chars ou em conflito recebem sufixo hash estavel dentro
    do limite;
  - guardar mapa `claude_name -> ToolName`;
  - combinar descricao do namespace com a descricao da ferramenta filha;
  - nunca tentar reconstruir namespace apenas por split de string.
- na volta, `tool_use.name` precisa existir no mapa;
- se nao existir, falhar fechado com erro de ferramenta claro.

Faseamento MCP:

- fase 2: function tools sem namespace;
- fase 3A: namespaces diretos achatados e restaurados;
- fase 3B: MCP local direto, sem deferred loading;
- fase 3C: deferred MCP/tool_search.

Ponto dificil, sem fingir que esta resolvido:

- Anthropic Messages recebe a lista de tools no request. O `tool_search` do
  Codex/Responses pode carregar ferramentas no meio do loop de um jeito que nao
  mapeia 1:1 para Anthropic.
- Plano para deferred:
  1. primeiro liberar modo "expandir tudo ate um budget" para MCP;
  2. se passar do budget, chamar um client tool `codex_tool_search`;
  3. depois do resultado, iniciar uma nova chamada Anthropic no mesmo turno com
     a lista de ferramentas expandida;
  4. testar que a UI continua vendo isso como uma unica interacao de usuario.
- Ate isso passar, nao marcar "MCP completo".

### Web search

O Codex hoje tem `ToolSpec::WebSearch` no formato OpenAI Responses. Anthropic
tambem tem server-side web search, mas o tipo, payload e output nao sao iguais.

Plano:

- fase inicial: `web_search=false` no provider Anthropic;
- fase web: criar adaptador provider-specific para server tool Anthropic, ou
  expor busca como client tool local do Codex;
- nao serializar `ToolSpec::WebSearch` OpenAI para Anthropic;
- so ligar capability quando houver fixture de request e output.

### Image generation

Claude Messages nao substitui `image_generation` da Responses API.

Plano:

- fase inicial: `image_generation=false`;
- se o Desktop/Codex tiver ferramenta local de imagem separada, ela deve ser
  exposta como client tool normal, nao como capability nativa do Claude;
- se depender de OpenAI image API, documentar como provider secundario, com
  chave separada e fallback claro.

### Tool search e plugins

`ToolSpec::ToolSearch` e deferred tool loading sao areas de maior risco.

Plano:

- nao declarar suporte total no MVP;
- manter plugins/skills instalados e registry intactos;
- permitir direct tools primeiro;
- so depois implementar mecanismo Anthropic de "search -> nova rodada com tools
  expandidas".

## Provider capabilities

Nao usar capabilities como marketing. Usar como contrato testado.

Estados:

1. `AnthropicTextOnly`
   - namespace_tools=false
   - image_generation=false
   - web_search=false
   - shell/apply_patch podem estar indisponiveis se expostos como namespace.

2. `AnthropicFunctionTools`
   - namespace_tools=false
   - image_generation=false
   - web_search=false
   - function tools sem namespace passam.

3. `AnthropicFlattenedNamespaces`
   - namespace_tools=true
   - image_generation=false
   - web_search=false
   - `ToolSpec::Namespace` e convertido no adapter, nunca enviado bruto.

4. `AnthropicFullCodexTools`
   - namespace_tools=true
   - web_search=true somente se web adapter passar;
   - image_generation=true somente se tool/provider separado passar;
   - deferred MCP/tool_search passa por loop de nova rodada.

## Auth e gateway

Gateway de terceiros pode ser apenas provider, mas tambem pode alterar:

- formato de tool use;
- headers;
- streaming;
- nomes de modelo;
- erros;
- limites de contexto;
- suporte a server tools.

Contrato:

- primeiro validar contra Anthropic Messages oficial ou mock fiel;
- depois validar gateway com a mesma suite de conformance;
- nunca escrever chave real em repo;
- config de exemplo usa placeholders;
- gateway so e suportado se passar:
  - texto streaming;
  - function tool;
  - tool_result;
  - erro/retry;
  - MCP direto quando essa fase existir.

Config minima esperada, sem segredo:

```toml
[model_providers.anthropic]
name = "Anthropic"
base_url = "https://api.anthropic.com/v1"
wire_api = "anthropic_messages"
requires_openai_auth = false
supports_websockets = false

[model_providers.anthropic.http_headers]
anthropic-version = "2023-06-01"

[model_providers.anthropic.env_http_headers]
x-api-key = "ANTHROPIC_API_KEY"
```

Para gateway:

```toml
[model_providers.claude-gateway]
name = "Claude Gateway"
base_url = "https://SEU_GATEWAY_AQUI"
wire_api = "anthropic_messages"
requires_openai_auth = false
supports_websockets = false

[model_providers.claude-gateway.http_headers]
anthropic-version = "2023-06-01"

[model_providers.claude-gateway.env_http_headers]
x-api-key = "CLAUDE_GATEWAY_API_KEY"
```

## Model catalog

OpenAI provider pode buscar `/models`. Anthropic/gateway talvez nao exponha
catalogo compativel.

Plano:

- usar catalogo estatico Anthropic no provider, como Amazon Bedrock ja faz;
- campos de `ModelInfo` precisam ser conservadores:
  - `shell_type`: o mesmo que o modelo Codex usa quando function tools estao
    prontos;
  - `apply_patch_tool_type`: habilitar so quando function/freeform adapter
    passar;
  - `supports_search_tool`: habilitado quando o adapter Anthropic de
    web/tool_search estiver validado;
  - `supports_parallel_tool_calls`: false ate provar chamadas paralelas;
  - `input_modalities`: text primeiro; image so depois de mapear Anthropic image
    input.
- `max_output_tokens`: catalogo Anthropic define `4096` como cap inicial de
  request. Este valor e um limite operacional conservador enviado em
  `max_tokens`, nao uma declaracao de maximo absoluto do modelo.

## Compaction e memoria

Risco: Codex usa `ResponseItem` historico e compaction pode depender de
endpoints OpenAI.

Evidencia local:

- `ModelProviderInfo::supports_remote_compaction()` retorna true para OpenAI ou
  Azure Responses, nao para provider arbitrario.

Plano:

- Anthropic nao deve usar remote compaction OpenAI;
- provar o caminho local existente antes de sessoes longas;
- se faltar caminho local bom, criar compaction Anthropic como endpoint separado
  usando o mesmo adaptador de Messages;
- nunca enviar `encrypted_content` OpenAI para Anthropic sem decisao explicita.

Aceite:

- conversa longa aciona compaction sem quebrar historico;
- tool calls antes e depois da compaction continuam com `call_id` correto;
- resumo nao apaga instrucoes de sandbox/permissao.

Estado atual:

- `ModelProviderInfo::supports_remote_compaction()` continua retornando `false`
  para Anthropic, logo o provider nativo nao usa os endpoints remotos OpenAI de
  compactacao;
- compactacao manual com Anthropic foi validada pelo caminho local existente:
  `Op::Compact` gera uma nova chamada `POST /v1/messages` com
  `SUMMARIZATION_PROMPT`;
- o request de compactacao Anthropic usa formato Messages (`messages[]`), nao
  payload Responses (`input`) nem endpoint compact remoto;
- compactacao automatica com tool calls antes/depois da compactacao tambem foi
  validada: o primeiro `tool_result` volta antes do compact, o compact usa
  `SUMMARIZATION_PROMPT`, e o segundo `tool_result` volta depois do resumo.

## Desktop vs CLI

O repo contem CLI, TUI e app-server. O shell Desktop em si pode depender de
partes fora deste clone. O plano nao deve prometer controle total do Desktop sem
provar o caminho de build/empacotamento.

Plano:

- implementar primeiro em `codex-rs` com CLI/TUI/app-server;
- validar `modelProvider/capabilities/read` porque o Desktop consulta
  capabilities via app-server;
- depois validar se o Desktop local consegue apontar para o binario/app-server
  modificado;
- se Desktop for fechado ou nao-buildavel, o fork entrega CLI/TUI/app-server e
  um guia de integracao, nao um Desktop completo.

## Fases com criterio de aceite

### Fase 0: Conformance harness

Objetivo: impedir que gateway ou Claude "pareca funcionar" so em prompt simples.

Mudar provavelmente:

- testes em `codex-rs/codex-api`;
- fixtures Anthropic SSE;
- docs de plano.

Aceite:

- teste parseia stream texto Anthropic;
- teste parseia stream tool_use com `input_json_delta` fragmentado;
- teste mapeia erro Anthropic para `ApiError`;
- nenhum codigo de UI alterado.

Rollback: remover modulo/testes Anthropic sem tocar harness.

### Fase 1: WireApi e texto streaming

Objetivo: `codex` conversa com Claude sem ferramentas.

Mudar provavelmente:

- `codex-rs/model-provider-info/src/lib.rs`;
- `codex-rs/model-provider/src/provider.rs`;
- `codex-rs/codex-api/src/endpoint/anthropic_messages.rs`;
- `codex-rs/codex-api/src/sse/anthropic.rs`;
- `codex-rs/core/src/client.rs` apenas para delegar.

Aceite:

- `wire_api = "anthropic_messages"` desserializa;
- request vai para `/messages`;
- stream de texto aparece na UI/CLI;
- erro de auth faltante aponta para env var certa;
- `just write-config-schema` se config schema mudar;
- `cargo test -p codex-model-provider-info`;
- `cargo test -p codex-api`;
- teste focado de `codex-core` para roteamento de WireApi.

Rollback: voltar `WireApi` e branch de client.

### Fase 2: Function tools sem namespace

Objetivo: function tools basicas via Anthropic client tools, com shell liberado
primeiro. `apply_patch` fica fora desta fase enquanto continuar modelado como
custom/freeform tool.

Mudar provavelmente:

- `codex-rs/codex-api` request adapter;
- `codex-rs/model-provider/src/anthropic/catalog.rs`;
- testes de stream/tool loop.

Aceite:

- `ToolSpec::Function` vira tool Anthropic valida;
- chamada `exec_command` despacha no mesmo handler;
- `tool_result` volta para Claude;
- segunda resposta chega no mesmo turno;
- JSON invalido em tool_use vira erro claro, nao panic;
- nao serializar `ToolSpec::Namespace`, `WebSearch`, `ImageGeneration` para
  Anthropic.

Estado atual:

- `ToolSpec::Function` e convertido para `tools[]` Anthropic;
- `tool_use` streaming com `input_json_delta` fragmentado vira
  `ResponseItem::FunctionCall`;
- historico `FunctionCall`/`FunctionCallOutput` volta como `tool_use`/
  `tool_result`;
- mensagens instrucionais `developer`/`system` do historico viram o campo
  `system` Anthropic, evitando role invalida em `/v1/messages`;
- teste de integracao cobre rodada completa:
  `tool_use(exec_command)` -> dispatcher do Codex -> `tool_result` no segundo
  request Anthropic -> segunda resposta no mesmo turno;
- `apply_patch` via heredoc dentro de `exec_command` foi comprovado: o
  dispatcher/intercept existente aplica o patch, grava o arquivo e devolve o
  resultado como `tool_result` para a segunda chamada Anthropic;
- JSON invalido em `tool_use` vira erro claro;
- chamadas hosted, custom/freeform e resultados de imagem seguem bloqueados;
- catalogo Anthropic habilita a familia de shell, mas ainda nao habilita
  `apply_patch` como tool freeform direta, web, imagem ou deferred tool search.

Rollback: desabilitar tools no provider e manter texto.

### Fase 3A: Namespaces achatados

Objetivo: suportar `ToolSpec::Namespace` sem mandar `type=namespace` ao Claude.

Mudar:

- `codex-rs/codex-api/src/anthropic_tool_names.rs`;
- `codex-rs/codex-api/src/endpoint/anthropic_messages.rs`;
- `codex-rs/codex-api/src/sse/anthropic.rs`;
- `codex-rs/model-provider/src/anthropic/mod.rs`;
- testes em `codex-rs/codex-api`;
- testes focados de integracao em `codex-rs/core/tests/responses_headers.rs`.

Aceite:

- nomes Claude-safe obedecem regex e limite de 64 chars;
- conflitos sao resolvidos por mapa, nao por split fragil;
- `namespace/name` volta exatamente ao `ToolRouter`;
- chamada namespaced passa pelo registry correto;
- ferramenta desconhecida falha fechada.

Estado atual:

- `ToolSpec::Namespace` e convertido para uma lista plana de client tools
  Anthropic, nunca serializado como `type=namespace`;
- o mapa reversivel vive no request Anthropic e e passado ao parser SSE;
- historico `ResponseItem::FunctionCall { namespace: Some(..) }` volta como
  `tool_use` com nome Anthropic-safe;
- `tool_use.name` recebido do stream e restaurado para `namespace/name` antes
  de chegar ao `ToolRouter`;
- conflitos e nomes longos recebem sufixo hash estavel;
- quando ha mapa ativo, nome desconhecido de `tool_use` falha fechado com erro
  de stream;
- capability Anthropic agora anuncia `namespace_tools=true`, mantendo
  `web_search=false` e `image_generation=false`;
- testes cobrem serializacao de namespace, retorno namespaced via SSE, colisao,
  limite de 64 chars e erro de tool desconhecida.

Rollback: capability `namespace_tools=false`.

### Fase 3B: MCP local direto

Objetivo: MCP local funciona quando ferramentas cabem no request.

Mudar:

- nenhum bypass do `ToolRouter`;
- fixtures Anthropic em `codex-rs/core/tests/responses_headers.rs`;
- harness de teste para aguardar startup MCP quando o binario de teste precisa
  ser construido antes do handshake STDIO.

Aceite:

- MCP STDIO/local listado pelo Codex vira tools Anthropic achatadas;
- chamada volta para `McpHandler`;
- output estruturado de MCP vira `tool_result`;
- paralelismo segue `supports_parallel_tool_calls`;
- sem uso do MCP connector remoto da Anthropic para substituir local.

Estado atual:

- MCP STDIO/local direto foi validado com o servidor real
  `test_stdio_server`;
- o primeiro request Anthropic recebe a tool MCP achatada, por exemplo
  `mcp__rmcp__echo`, sem `type=namespace` bruto;
- o stream Anthropic com `tool_use.name = "mcp__rmcp__echo"` e restaurado para
  `namespace = "mcp__rmcp__"` + `name = "echo"`;
- o `ToolRouter` despacha para o handler MCP existente;
- o output estruturado do MCP volta para Anthropic como `tool_result` textual
  no segundo request;
- nenhum MCP connector remoto da Anthropic foi introduzido;
- deferred MCP/tool_search segue fora desta fase.

Rollback: manter MCP desabilitado para Anthropic.

### Fase 3C: Deferred MCP e tool_search

Objetivo: recuperar comportamento de grande catalogo de ferramentas.

Mudar provavelmente:

- adapter Anthropic para expor `tool_search` como client tool
  `codex_tool_search`;
- parser SSE Anthropic para restaurar `codex_tool_search` como
  `ToolSearchCall` client-side;
- adapter Anthropic para transformar `tool_search_output` em definicoes de
  tools expandidas no request seguinte;
- catalogo Claude com `supports_search_tool=true`;
- testes de multi-request no mesmo turno.

Aceite:

- quando tools excedem budget, Claude recebe `codex_tool_search`;
- busca retorna definicoes relevantes;
- Codex inicia nova chamada com tools expandidas sem criar nova mensagem de
  usuario;
- UI mostra progresso coerente;
- chamada da tool recem-expandida despacha corretamente.

Estado atual:

- Claude recebe `codex_tool_search` em vez de `tool_search` bruto;
- `tool_use.name = "codex_tool_search"` volta como `ResponseItem::ToolSearchCall`
  com `execution = "client"`;
- `tool_search_output` client-side e replayado como `tool_result` textual e
  tambem expande as definicoes de ferramentas no request Anthropic seguinte;
- MCP STDIO/local deferred foi validado com `ToolSearchAlwaysDeferMcpTools` e
  servidor real `test_stdio_server`;
- o primeiro request esconde `mcp__rmcp__echo`, o segundo inclui a tool
  expandida, e a chamada subsequente despacha pelo `McpHandler`;
- `supports_search_tool=true` esta ativo no catalogo estatico Claude;
- ainda nao cobre tool_search de apps/connector remoto no caminho Anthropic.

Rollback: fallback "expandir tudo ate budget e avisar limite".

### Fase 4: Web

Objetivo: busca na web com semantica documentada.

Opcoes:

- Anthropic server tool `web_search`;
- client tool local do Codex;
- manter provider secundario.

Aceite:

- capability `web_search=true` so depois de teste real/mock;
- resultados/citacoes nao quebram `ResponseItem`;
- custos e dependencia de provider ficam documentados.

Estado atual:

- caminho Anthropic usa o server tool basico `web_search_20250305`, conforme
  documentacao Claude atual;
- `web_search_20260209` ficou fora de escopo porque exige code execution para
  dynamic filtering;
- `web_search = "live"` e serializado como `{ type = "web_search_20250305",
  name = "web_search" }`;
- `web_search = "cached"` nao e enviado para Anthropic, para nao transformar
  uma busca cached em busca live silenciosamente;
- `server_tool_use` + `web_search_tool_result` no stream Anthropic viram
  `ResponseItem::WebSearchCall` sem quebrar texto posterior;
- `web_search_tool_result.content` e preservado como `results` bruto em
  `WebSearchCall`/itens internos; interpretacao semantica fina de citacoes fica
  para uma fase propria.

Rollback: `web_search=false`.

### Fase 5: Imagem

Objetivo: nao prometer imagem Claude-native se ela vier de outro lugar.

Aceite:

- se houver geracao, ela e tool/provider separado;
- config separa chave/modelo;
- erro deixa claro qual provider falhou.

Estado atual:

- Anthropic nativo continua com `image_generation=false`;
- o catalogo Claude nao anuncia `image_generation`;
- o adapter Anthropic rejeita `ToolSpec::ImageGeneration` com erro de request
  invalido se alguma configuracao externa tentar forcar essa tool;
- nenhum provider/tool separado de imagem foi introduzido nesta fase.

Rollback: `image_generation=false`.

### Fase 6: Empacotamento e perfil padrao

Objetivo: usuario consegue usar sem memorizar setup.

Aceite:

- profile exemplo sem segredo;
- README curto com env vars;
- smoke test CLI/TUI;
- se Desktop for integrado, validar app-server capabilities;
- matriz "oficial Anthropic" vs "gateway" preenchida.

Estado atual:

- o provider embutido `anthropic` esta registrado em
  `built_in_model_providers`;
- o contrato do provider embutido esta coberto por teste:
  `wire_api = "anthropic_messages"`, `ANTHROPIC_API_KEY`,
  `anthropic-version = "2023-06-01"` e `is_anthropic()`;
- auth Anthropic esta coberto por teste: env key ausente gera erro apontando
  para `ANTHROPIC_API_KEY`, e env key presente e enviado como `x-api-key`, sem
  `Authorization: Bearer`;
- o perfil minimo continua sem segredo versionado:
  `model_provider = "anthropic"` e `model = "claude-sonnet-4-6"`;
- capacidades do provider Anthropic oficial ficam baseadas no que ja tem teste:
  namespaces/MCP/tool_search/live web habilitados, imagem e apply_patch direto
  desabilitados;
- app-server valida `modelProvider/capabilities/read` para o provider
  `anthropic`: `namespace_tools=true`, `web_search=true` e
  `image_generation=false`;
- app-server valida `model/list` para o provider `anthropic` usando o catalogo
  estatico Claude, sem depender de chamada `/models`;
- app-server valida smoke path `thread/start` + `turn/start` com provider
  Anthropic customizado apontando para mock local: o request usa
  `POST /v1/messages`, header `x-api-key`, `anthropic-version`, formato
  Messages (`messages[]`) e completa um item `AgentMessage`;
- `codex-exec` valida smoke CLI contra mock Anthropic Messages: config
  `wire_api = "anthropic_messages"`, env `ANTHROPIC_API_KEY`, request para
  `/v1/messages`, header `x-api-key`, sem `Authorization`, e body com
  `messages[]`;
- smoke TUI fica manual/ignorado porque depende de `tmux` e binario local, mas
  agora existe teste dedicado para Anthropic Messages com mock `/v1/messages`;
- README externo ou documentacao geral ainda nao foi criada de proposito; este
  arquivo segue como artefato operacional para evitar documentacao prematura
  fora do escopo do fork.

Handoff operacional de uso:

1. Exportar a chave no ambiente local, sem versionar segredo:

   ```bash
   export ANTHROPIC_API_KEY=...
   ```

2. Usar o perfil minimo no `config.toml`:

   ```toml
   model_provider = "anthropic"
   model = "claude-sonnet-4-6"
   ```

3. O provider embutido resolve o restante do contrato oficial:

   - `base_url = "https://api.anthropic.com/v1"`;
   - `wire_api = "anthropic_messages"`;
   - request para `POST /v1/messages`;
   - auth via header `x-api-key`;
   - header `anthropic-version = "2023-06-01"`;
   - catalogo estatico Claude, sem depender de `/models`.

4. Recursos considerados suportados neste ponto sao apenas os que ja tem prova
   local/mock:

   - texto streaming;
   - function tools basicas;
   - namespace tools e MCP STDIO/local via flatten/restore;
   - deferred `codex_tool_search`;
   - web live via server tool Anthropic;
   - compactacao pelo caminho local;
   - app-server `modelProvider/capabilities/read`, `model/list` e smoke path
     `thread/start` + `turn/start`;
   - `codex-exec` com mock Anthropic Messages;
   - TUI manual via `tmux` com mock Anthropic Messages.

5. Limites explicitos:

   - imagem Claude-native continua desabilitada;
   - `apply_patch` direto/freeform nao e anunciado para Anthropic;
   - compactacao remota OpenAI nao e usada pelo provider Anthropic;
   - gateway so deixa de ser experimental depois de passar a mesma conformance
     do Anthropic oficial.

6. Smoke manual recomendado antes de entregar para uso real:

   - confirmar `ANTHROPIC_API_KEY` no ambiente do processo;
   - iniciar uma conversa texto curta;
   - executar uma chamada simples de shell;
   - rodar o smoke TUI manual quando houver `tmux` e binario local;
   - validar que capabilities do app-server continuam reportando
     `namespace_tools=true`, `web_search=true` e `image_generation=false`.

Matriz oficial Anthropic vs gateway:

| Caminho | Configuracao | Contrato atual | Status |
| --- | --- | --- | --- |
| Anthropic oficial | provider embutido `anthropic`, `wire_api = "anthropic_messages"`, `ANTHROPIC_API_KEY`, `x-api-key`, `anthropic-version = "2023-06-01"` | request/stream Messages nativo; function tools, namespace/MCP, deferred `tool_search`, web live; imagem e remote compaction OpenAI desabilitados | Suportado pelo adapter nativo e testes locais/mock |
| Gateway OpenAI-compatible | provider customizado, normalmente `wire_api = "responses"` ou contrato equivalente do gateway | pode funcionar para texto simples, mas precisa provar tools especiais, namespaces, MCP, web, auth e streaming | Experimental ate passar a mesma suite de conformance do Anthropic oficial |

Rollback: remover o provider embutido `anthropic` e voltar para configuracao
customizada explicita enquanto o adapter nativo continua em teste.

## Matriz de riscos

| Risco | Por que importa | Mitigacao |
| --- | --- | --- |
| Enviar `ToolSpec::Namespace` bruto ao Claude | Anthropic nao aceita o tipo OpenAI | Adapter provider-specific e testes de serializacao |
| Perder namespace no retorno | MCP/plugin errado pode ser chamado | `ToolNameMap` obrigatorio por request |
| Gateway mentir compatibilidade | Pode passar texto e quebrar tools | Suite de conformance antes de suporte |
| Auth errada | `env_key` atual tende a Bearer; Anthropic usa `x-api-key` | `env_http_headers` ou auth scheme novo |
| `max_tokens` chutado | Anthropic exige e afeta custo/saida | Resolvido por `ModelInfo.max_output_tokens` e catalogo Anthropic explicito |
| Deferred tools sem equivalente | Pode quebrar "tudo que Codex faz" | Fase propria com nova rodada no mesmo turno |
| Imagem assumida como Claude-native | Funcionalidade falsa | Provider/tool separado ou disabled |
| Desktop fora do repo | Fork pode nao empacotar app final | Validar app-server primeiro e marcar limite |
| Crescer `codex-core` | Drift contra AGENTS.md | Wire em `codex-api`, tools em `codex-tools`, core so delega |
| Reasoning/encrypted OpenAI | Formato nao-portavel | Nao enviar para Anthropic sem decisao |

## Ordem de leitura para proximo agente

1. Este arquivo.
2. `codex-claude-fork/openai-codex-upstream/AGENTS.md`.
3. `codex-rs/codex-api/README.md`.
4. `codex-rs/tools/README.md`.
5. `codex-rs/model-provider-info/src/lib.rs`.
6. `codex-rs/model-provider/src/provider.rs`.
7. `codex-rs/core/src/client.rs`.
8. `codex-rs/core/src/tools/spec_plan.rs`.
9. `codex-rs/tools/src/tool_spec.rs`.
10. `codex-rs/core/src/tools/router.rs`.

## Primeiro PR recomendado

Nao comecar por MCP, web ou imagem.

Escopo:

- `WireApi::AnthropicMessages`;
- provider config sem segredo;
- request/stream Anthropic texto-only;
- fixtures SSE;
- branch minima em `ModelClient::stream`;
- static model catalog minimo;
- docs de config exemplo.

Nao incluir:

- MCP;
- deferred tool search;
- web;
- image;
- Desktop packaging;
- gateway-specific hacks.

## Split atual recomendado para PRs

O estado atual do working tree ja passou do "primeiro PR" acima. Para revisao,
nao enviar tudo em um PR unico.

Base de evidencia:

- o evidence pack padrao tentou usar `baseline-2026-01-27`, mas essa ref nao
  existe neste clone;
- enquanto uma BASE oficial nao for informada, tratar `origin/main` apenas como
  base pratica de split local, nao como pacote formal de evidencia da skill;
- antes de abrir PR, gerar o pacote formal com uma BASE explicita e patch em
  `/tmp`.

PR 1: provider/wire Anthropic Messages texto

- incluir `WireApi::AnthropicMessages`, client `/v1/messages`, parser SSE texto,
  provider/catalogo Anthropic e auth `x-api-key`;
- incluir modelo default/catalogo estatico, schema/config necessario e smoke de
  texto;
- nao incluir MCP, web, app-server nem TUI;
- checks: `cargo test -p codex-model-provider-info`, `cargo test -p codex-api`
  e teste focado de `codex-core` para request texto.

PR 2: ferramentas client-side e nomes Anthropic-safe

- incluir function tools, shell pelo dispatcher existente, intercept de
  `apply_patch` via heredoc, `AnthropicToolNameMap`, namespace tools e MCP
  STDIO/local direto;
- incluir deferred `codex_tool_search` somente se o diff ficar revisavel; se
  crescer demais, separar em PR 2B;
- checks: testes focados de `codex-core` para function tool, apply_patch,
  namespace/MCP e tool_search.

PR 3: web live e eventos/protocolo

- incluir server tool Anthropic `web_search_20250305`;
- incluir preservacao de `web_search_tool_result.content` em `results` nos
  tipos de protocolo, app-server schema, exec JSON e replay;
- checks: `cargo test -p codex-protocol web_search -- --nocapture`,
  `cargo test -p codex-core web_search -- --nocapture`,
  `cargo test -p codex-exec web_search -- --nocapture` e
  `cargo test -p codex-app-server-protocol`.

PR 4: empacotamento/smokes Fase 6

- incluir app-server capabilities/model list/turn smoke, smoke CLI
  `codex-exec`, smoke TUI manual/ignorado e este handoff operacional;
- checks: `cargo test -p codex-app-server --test all anthropic -- --nocapture`,
  `cargo test -p codex-exec anthropic_messages_provider_smoke -- --nocapture`
  e compilacao do smoke TUI sem `--ignored`.

## Config exemplo da Fase 1

O provider Anthropic nativo fica disponivel como `anthropic` e usa
`ANTHROPIC_API_KEY` via header `x-api-key`.

```toml
model_provider = "anthropic"
model = "claude-sonnet-4-6"
```

Estado da Fase 1:

- `wire_api = "anthropic_messages"` chama `POST /v1/messages`;
- `anthropic-version = "2023-06-01"` e configurado pelo provider embutido;
- stream de texto e convertido para os eventos internos do Codex;
- function tools basicas da Fase 2 ja tem prova de stream, dispatch e
  `tool_result`;
- `apply_patch` ja tem prova pelo caminho conservador de `exec_command` heredoc,
  sem anunciar a tool freeform Anthropic;
- namespaces diretos da Fase 3A ja sao achatados/restaurados com mapa
  reversivel;
- MCP STDIO/local direto da Fase 3B ja foi validado com servidor real e
  dispatcher existente;
- deferred MCP/tool_search da Fase 3C ja foi validado com `codex_tool_search`,
  tool expansion no request seguinte e dispatcher MCP existente;
- web live da Fase 4 ja usa `web_search_20250305`; web cached nao e anunciado
  para Anthropic;
- imagem da Fase 5 segue explicitamente desabilitada para Anthropic nativo;
- `apply_patch` direto continua desabilitado no
  catalogo/capabilities.

## Claudex launcher isolado

`claudex` e um software separado do ponto de vista de instalacao e identidade:
tem entrypoint nativo, pacote npm meta proprio e home proprio. Ele resolve
`CLAUDEX_HOME` e injeta esse valor como `CODEX_HOME` no processo filho; se
`CLAUDEX_HOME` nao existir, usa `~/.claudex`.

Uso esperado:

```bash
claudex --version
claudex exec "Reply OK."
CLAUDEX_HOME=/tmp/claudex-home claudex app-server --listen stdio://
```

Contratos:

- nao le nem grava `~/.codex` por padrao;
- cria o diretorio de home antes de iniciar o processo filho;
- aceita `CLAUDEX_CODEX_BIN` apenas como escape hatch de teste/dev para apontar
  para um binario Codex especifico;
- o pacote npm `claudex` expoe somente o bin `claudex`;
- o pacote `@openai/codex` nao expoe `claudex`;
- o package builder tambem aceita `--variant claudex` para criar pacote nativo
  com `bin/claudex`.
- `claudex app` nao delega para `codex app`: quando houver shell Desktop
  Claudex, ele deve abrir/procurar `Claudex`/`Claudex.app`; em plataformas ou
  builds sem esse artefato, falha explicitamente em vez de abrir Codex Desktop.

## Checks obrigatorios por tipo de mudanca

Sempre:

- `git status --short --branch`;
- `just fmt` em `codex-rs` se Rust mudou;
- `git diff --check`.

Se `ModelProviderInfo`, `ConfigToml` ou schema mudar:

- `just write-config-schema`.

Se deps mudarem:

- `just bazel-lock-update`;
- `just bazel-lock-check`.

Por fase:

- fase 1: `cargo test -p codex-model-provider-info`, `cargo test -p codex-api`;
- fase 2/3: `cargo test -p codex-tools`, testes focados de `codex-core`;
- fase 6 CLI: `cargo test -p codex-exec anthropic_messages_provider_smoke -- --nocapture`;
- fase 6 TUI manual: `cargo test -p codex-tui tmux_anthropic_messages_provider_renders_response -- --ignored --nocapture`;
- mudanca em core/protocol/common: depois dos focados, pedir confirmacao antes
  de suite completa se ela for pesada;
- UI/TUI visivel: snapshot tests do `codex-tui`.

## Definicao de "pronto"

O fork so pode ser chamado de "Codex com Claude" quando:

- uma conversa texto funciona via `anthropic_messages`;
- pelo menos uma function tool executa e retorna resultado;
- shell e apply_patch passam pelo dispatcher normal;
- namespace/MCP passam por `ToolNameMap`, sem nomes brutos OpenAI;
- capabilities reportam somente recursos testados;
- historico com tool_use/tool_result sobrevive a mais de uma rodada;
- compaction nao depende de OpenAI remote endpoint;
- config nao contem segredo;
- gateway passa a mesma suite que Anthropic oficial ou fica marcado como
  experimental.

Enquanto isso nao acontecer, o nome correto e:

- "experimento texto Claude", ou
- "Claude provider parcial", ou
- "fork em fase N".
