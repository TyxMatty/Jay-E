# Jay-E — Estudo do LibreOffice, ONLYOFFICE, MarkText, PhotoCraft e Kdenlive

Notas da leitura do código de cinco projetos open-source, feita a pedido do usuário antes de escrever o Jay-E. O Jay-E é **MIT** (D1). De LibreOffice (MPL-2.0/LGPL), ONLYOFFICE (AGPL-3.0) e Kdenlive (GPL-3.0) saem ideias, organização e números, **nunca código**. MarkText (MIT) e PhotoCraft (MIT/Apache-2.0) têm licença compatível; mesmo assim, só ideias por enquanto, e um trecho copiado de um deles leva o aviso de licença junto. Os caminhos citados são dos repositórios deles.

Lido em 2026-11-06: LibreOffice `core` `72b0b19` (2026-11-06), ONLYOFFICE `core` `ed63e94` e `sdkjs` `9d21388` (2026-10-15), MarkText `a520bef` (2026-11-06), PhotoCraft `b49c6c9` (2026-11-06), Kdenlive `4a4d449` (2026-11-06). Clones rasos e parciais: do LibreOffice e do ONLYOFFICE, só os READMEs, cabeçalhos e listas de arquivos citados.

## 1. LibreOffice (MPL-2.0, partes LGPL-3.0)

A suíte inteira (Writer, Calc, Impress, Draw), em C++, com a UNO como modelo de componentes. Mais de 150 módulos de primeiro nível.

### 1.1 Tipo e filtro são coisas separadas

- `filter/README.md`: a configuração de filtros tem duas partes, os **tipos** (`filter/source/config/fragments/types/`) e os **filtros** (`.../filters/`). Cada tipo de arquivo tem **exatamente uma** definição de tipo, e vários filtros podem usá-la (ex.: texto puro abre no Writer e no Calc).
- A detecção olha o conteúdo, não só a extensão.
- **No Jay-E:** `jay_docs::detect` decide o tipo pelos primeiros bytes (a extensão só desempata contêineres iguais, como ZIP → DOCX ou ODT); o visualizador vem depois, escolhido pelo tipo. Feito na etapa 0.

### 1.2 Leitor = tokenizador + mapeador + modelo

- `sw/README.writerfilter.md`: a importação de DOCX e RTF tem **tokenizadores** por formato (`ooxml/`, `rtftok/`) que alimentam um **domain mapper** comum (`dmapper/`), que escreve no modelo do Writer. O mapeador esconde do tokenizador a API do documento.
- Para depurar, o tráfego que entra no `dmapper` pode ser despejado num XML (`SW_DEBUG_WRITERFILTER=1`).
- **No Jay-E:** cada leitor (DOCX, ODT, RTF, Markdown) só traduz eventos do formato para um mapeador comum que monta o modelo interno (seção 2.1, a mesma ideia). Um despejo do modelo em JSON serve de teste de regressão.

### 1.3 LibreOfficeKit: renderização em tiles

- `libreofficekit/README.md` e `include/LibreOfficeKit/LibreOfficeKit.hxx`: uma API C sem UNO para converter documentos (`documentLoad`, `saveAs(url, formato, opções)`) e, como API "instável", **renderizar em tiles**: `paintTile(buffer, larguraDoCanvas, alturaDoCanvas, x, y, larguraDoTile, alturaDoTile)`, com a posição em coordenadas do documento (twips) e o buffer em BGRA de 32 bits, linhas de cima para baixo. `getDocumentSize`, `getParts`/`setPart` (páginas ou planilhas).
- A edição por tiles é um canal de duas mãos: o cliente manda teclas e cliques (`postKeyEvent`, `postMouseEvent`), e o LibreOffice avisa por callback quais retângulos invalidou.
- É o que o Collabora Online usa no navegador.
- **No Jay-E:** o visualizador de PDF (e depois o de imagens grandes) pede **tiles** ao Rust por coordenada e zoom, e só redesenha os retângulos invalidados. O Rust entrega o tile pronto pelo protocolo próprio (seção 5 do plano), sem passar pixels pelo IPC em JSON.

### 1.4 PDF

- O LibreOffice usa o **PDFium** para ler e desenhar PDF (`include/vcl/filter/PDFiumLibrary.hxx`, um invólucro C++ dele), e tem um gravador de PDF próprio (`vcl/inc/pdf/pdfwriter_impl.hxx`, com criptografia R6, XMP, fontes embutidas).
- A **importação de PDF como desenho** (`sdext/README.md`) roda o poppler num **processo externo**: um PDF malicioso que derruba o parser não derruba o editor.
- **No Jay-E:** o PDFium foi medido contra o hayro (Rust puro) no spike S1, e o hayro ganhou (`ARQUITETURA.md`, seção 8). Com ele, um PDF malicioso não corrompe memória, então o parser fica no mesmo processo, numa thread por documento (D5).

### 1.5 Outros pontos

- Conversão sem janela: `soffice --headless --convert-to pdf arquivo.docx`. Serve de referência para conferir a saída do Jay-E nos testes, não como dependência (são ~300 MB).
- O LibreOffice já tem um `rust_uno/` (binding Rust da UNO, MPL-2.0), sinal de que o projeto está aceitando Rust, mas ele depende da instalação inteira.

## 2. ONLYOFFICE (AGPL-3.0)

Editores de documento, planilha e apresentação. O servidor (`core`, em C++) converte; o editor (`sdkjs`, em JavaScript) desenha tudo num `<canvas>`.

### 2.1 Conversão em estrela: um formato no meio

- `X2tConverter/src/ASCConverters.h` tem as rotas: `docx2doct`, `doct2docx`, `doct2rtf`, `doc2doct`, `html2doct`, `odf2oox`, `bin2pdf`, `md2html`, `hwp2docx`, `epub2docx_dir`, `fb2docx`… Quase tudo passa por um **formato interno** (`doct`, o `Editor.bin`) ou pelo DOCX: cada formato novo precisa de um leitor e um gravador, não de um conversor para cada outro formato (2N em vez de N²).
- O mesmo formato interno é o que o editor carrega: abrir e converter usam o mesmo caminho.
- **No Jay-E:** um modelo interno de documento (blocos: títulos, parágrafos com trechos formatados, listas, tabelas, imagens, código, fórmulas), com um leitor e um gravador por formato. Abrir para ler, editar e converter passam todos por ele. É a decisão central do plano (seção 6).

### 2.2 PDF → DOCX reconstruindo a estrutura

- `DocxRenderer/src/logic/`: para transformar PDF em DOCX, junta glifos em `TextLine`, linhas em `Paragraph`, retângulos em `Table` e `Shape` (`elements/`), com gerentes de fonte e de estilo (`managers/`).
- **No Jay-E:** PDF → DOCX/Markdown fica para depois do v1 (é o conversor mais difícil e o que mais erra); se entrar, segue essa ordem: glifos → linhas → parágrafos → tabelas.

### 2.3 Editor em canvas

- `sdkjs/word/` tem o próprio motor de layout e paginação; o texto é desenhado no canvas, sem `contentEditable`. Assim a página sai igual em qualquer navegador, mas é um motor de texto inteiro (fontes, quebra de linha, bidi, hifenização).
- `sdkjs/pdf/src/viewer.js` (5 900 linhas): visualizador de PDF com o motor compilado para WebAssembly (`pdf/src/engine/drawingfile.wasm`), um **cache de páginas** (`CCacheManager`), só as páginas visíveis são pedidas e desenhadas (`startVisiblePage`/`endVisiblePage`), miniaturas repintadas uma a uma, modos de zoom (largura, página, livre).
- **No Jay-E:** o visualizador de PDF copia essa organização (páginas visíveis, cache com limite, miniaturas sob demanda, modos de zoom), com o motor no Rust em vez de wasm. A edição de documentos **não** reimplementa um motor de layout: usa o HTML da WebView2 em páginas, que já faz fontes, bidi e quebra de linha (seção 6). Fica menos fiel ao Word que o ONLYOFFICE, e isso é aceito (risco 3).

## 3. MarkText (MIT)

Editor de Markdown WYSIWYG em Electron + Vue 3. O motor de edição é o **Muya** (`packages/muya`, TypeScript).

### 3.1 O motor

- O Markdown vira um **estado em JSON** (blocos) e volta para Markdown (`state/markdownToState.ts`, `state/stateToMarkdown.ts`); o HTML sai do estado (`markdownToHtml.ts`, `renderToStaticHTML.ts`).
- Desenha os blocos com **snabbdom** (DOM virtual) e só refaz o bloco que mudou; o resto da página fica.
- **Desfazer** com operações **ot-json1** sobre o estado (`history/index.ts`): pilha de no máximo 100, juntando digitação dentro de 1 s, guardando a seleção junto; operações no documento inteiro pedem reconstruir tudo.
- Conformidade testada contra as especificações **CommonMark 0.31 e GFM** (`test/spec/`).
- Matemática com **KaTeX**, diagramas com Mermaid/PlantUML/Vega, HTML embutido limpo pelo **DOMPurify**, com testes de segurança (`e2e/tests/security`).

### 3.2 Arquivos

- `main/filesystem/encoding.ts`: detecta a codificação (UTF-8, BOM, e o `ced` quando não é UTF-8) e **salva na mesma codificação**, para não trocar bytes por `U+FFFD`. Latin-1 é tratado como CP1252.
- `main/filesystem/watcher.ts`: vigia o arquivo e a pasta (chokidar) e avisa quando outro programa muda o arquivo aberto.
- `main/filesystem/documentWrites.ts`: as gravações de um documento entram numa fila; "só leitura" e barreiras de IO bloqueiam gravações novas até as anteriores terminarem.
- Exportar PDF é o `printToPDF` do Chromium sobre o HTML.
- Modos de foco, máquina de escrever e código-fonte.

### 3.3 Para o Jay-E

- **Markdown → HTML no Rust** (`pulldown-cmark`, que também passa nos testes CommonMark) e limpeza no Rust (`ammonia`), em vez de `marked` + DOMPurify no JavaScript.
- **Editor:** primeiro código-fonte com prévia ao lado (etapa 2), WYSIWYG por blocos depois (decisão aberta D4). O WYSIWYG do Muya é o melhor modelo para isso, e é MIT.
- **Salvar com segurança:** mesma codificação, mesmo fim de linha, gravação atômica (arquivo `.tmp` e renomear), vigiar mudanças de fora.
- **PDF pela própria WebView2**, como o `printToPDF`: o Markdown, o DOCX e o HTML viram PDF sem biblioteca nova.

## 4. PhotoCraft (MIT ou Apache-2.0)

Editor de imagem "em Rust puro", reimplementação do Photoshop feita em sala limpa (só especificações públicas e comportamento observado). 24 crates, interface em egui, compositor em wgpu.

### 4.1 Organização

- **Motor primeiro** (`README.md`, "Under the hood"): um modelo de documento só de dados (`crates/doc`) e um **motor de comandos** (`crates/engine`); a interface é uma camada fina por cima. Toda ação é um comando, chamável pela UI, pela CLI (`apps/photocraft-cli`), por um canal JSON e por um servidor MCP.
- **Camadas checadas no build** (`AGENTS.md`, `cargo xtask layers`): cada crate só depende das de baixo; `codecs`, `psd` e `cms` não dependem de nada do workspace; nada abaixo da UI usa egui.
- **Dois compositores**: um na CPU, que é a referência, e um na GPU, testados um contra o outro.
- **Tiles de 256² com cópia na escrita**: desfazer barato e imagens enormes leves.

### 4.2 Codecs

- Decodificadores em Rust: `image` (GIF, BMP, TGA, ICO, QOI, HDR), `png`, `zune-jpeg`, `jpeg-encoder`, `image-webp`, `tiff`, `exr`, JPEG XL lossless (`zune-jpegxl`); HEIF à parte, opcional.
- **Arquivo é entrada não confiável**, com limites explícitos em cada formato (`crates/codecs/src/codecs/`): `MAX_GIF_FRAMES = 100 000`, `MAX_IDAT = 1 GiB` no PNG, `MAX_IFDS = 10 000` e profundidade 4 de sub-IFD no TIFF, até 2 048 pixels por byte comprimido no JPEG (bomba de descompressão), ICC limitado no ICO.
- Recursão sempre limitada (`AGENTS.md`: documentos podem ser fundos ou cíclicos).

### 4.3 Para o Jay-E

- O `jay-docs` sem Tauri, com a interface fina por cima, como o `jay-audio` do Jay-A: a mesma ideia do motor de comandos. As operações de edição (cortar, girar, converter) são comandos de dados, testáveis sem janela.
- Os mesmos codecs em Rust para as imagens (etapa 3), com limites por formato e um limite de pixels total.
- Tiles para imagens grandes, compartilhando o caminho dos tiles de PDF.
- O que **não** entra: camadas, máscaras, pincéis e PSD. O Jay-E edita imagem no nível de "ajustar e converter" (cortar, girar, redimensionar, brilho/contraste, anotar), não de pintura.

## 5. Kdenlive (GPL-3.0)

Editor de vídeo do KDE, em C++/Qt. O trabalho pesado é do **MLT** (decodificar, aplicar efeitos, renderizar), que usa FFmpeg, frei0r, LADSPA, SoX.

### 5.1 Organização

- `dev-docs/architecture.md`: o Kdenlive é o **modelo e a interface**; o MLT faz o vídeo.
- **Projeto não destrutivo** (`dev-docs/fileformat.md`): o `.kdenlive` é um XML do MLT que só **referencia** a mídia; o MLT renderiza o arquivo de projeto direto, ignorando os dados extras do Kdenlive. Já houve um formato com dados duplicados que saíam de sincronia (geração 1): a lição é guardar cada dado num lugar só.
- Linha do tempo (`src/timeline2/model/`): trilhas, clipes, composições, grupos e *snap* como modelos separados.
- **Desfazer funcional** (`src/undohelper.hpp`): cada operação monta um par de funções desfazer/refazer (`Fun`), encadeadas com `PUSH_LAMBDA`; uma operação composta é a soma das pequenas.
- **Tarefas em segundo plano** (`src/jobs/`): carregar clipe, miniaturas, níveis de áudio, *proxy*, transcodificar, cortar, estabilizar, detectar cenas; um `taskmanager` com fila.
- **Proxy**: uma cópia leve do clipe para editar com fluidez, trocada pelo original na renderização (`kdenlivesettings.kcfg`: `enableproxy`, `generateproxy`).
- **Renderização em outro processo** (`renderer/kdenlive_render.cpp`): exportar um vídeo longo não trava a interface, e uma falha não derruba o editor.

### 5.2 Para o Jay-E

- O Jay-E **não** é um editor de vídeo com trilhas. Para vídeo: assistir, **cortar sem recodificar** (cópia de stream, instantâneo), juntar pedaços, extrair um quadro como imagem, converter. Uma trilha só.
- Do Kdenlive vêm: edição não destrutiva (lista de cortes, o original não muda), tarefas numa fila com progresso e cancelar, conversão em outro processo, miniaturas e forma de onda calculadas em segundo plano, e o desfazer como pares de funções.
- O motor de vídeo é a decisão mais pesada (D3): o FFmpeg tem 80–130 MB; o **Media Foundation** já vem no Windows (H.264, HEVC com extensão, AAC, MP4) e não pesa nada no instalador.

## 6. O que o Jay-E adota

| Ideia | De onde | Onde entra no Jay-E |
|---|---|---|
| Tipo detectado pelo conteúdo, separado do leitor | LibreOffice | `jay_docs::detect` (etapa 0 ✅) |
| Um modelo interno no meio: um leitor e um gravador por formato | ONLYOFFICE (`doct`) | o modelo de documento (etapa 4) e a conversão (etapa 8) |
| Leitor = tokenizador do formato + mapeador comum | LibreOffice (`writerfilter`) | os leitores de DOCX, ODT, RTF e Markdown |
| Tiles pedidos por coordenada e zoom, só o que está visível, cache com limite | LibreOfficeKit, ONLYOFFICE (`viewer.js`), PhotoCraft | visualizador de PDF (etapa 1) e de imagens grandes (etapa 3) |
| PDF desenhado no Rust; o parser isolado do resto | LibreOffice (PDFium, `pdfimport` em outro processo) | etapa 1 ✅, com o hayro (Rust seguro) numa thread por documento em vez de outro processo |
| Markdown → estado → Markdown, conformidade CommonMark/GFM, HTML limpo | MarkText | etapa 2, com `pulldown-cmark` e `ammonia` no Rust |
| Salvar na mesma codificação e fim de linha, gravação atômica, vigiar mudanças de fora | MarkText | todo editor de texto (etapa 2) |
| PDF pela impressão do navegador | MarkText (`printToPDF`) | exportar Markdown, DOCX e HTML para PDF |
| Motor sem interface, ações como comandos testáveis | PhotoCraft | o crate `jay-docs` |
| Arquivo é entrada não confiável, limites por formato, recursão limitada | PhotoCraft | todos os leitores |
| Codecs de imagem em Rust | PhotoCraft | etapa 3 |
| Edição não destrutiva: lista de edições, o original não muda | Kdenlive | imagem (etapa 3) e vídeo (etapa 5) |
| Desfazer como pares de funções; desfazer com a seleção junto | Kdenlive, MarkText | todos os editores |
| Tarefas em fila com progresso e cancelar; conversão em outro processo | Kdenlive | conversão (etapas 5 e 8) |

**Não adota:** a UNO e a suíte inteira do LibreOffice (planilhas e apresentações ficam fora do v1, D2), o motor de layout em canvas do ONLYOFFICE, a edição colaborativa, as camadas e pincéis do PhotoCraft, as trilhas múltiplas e os efeitos de vídeo do Kdenlive, e o Electron do MarkText (o Jay-E é Tauri, como a linha).
