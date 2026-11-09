# Roteiro do Jay-E

**Português** · [English](ROADMAP.en.md)

O Jay-E é o editor e leitor da linha Jay: documentos, imagens e vídeo, com conversão entre formatos. Windows primeiro.

Cada etapa termina com algo testável. O plano completo, com os testes de "pronto quando" e as medidas de cada etapa feita, está em [`docs/ARQUITETURA.md`](docs/ARQUITETURA.md), seção 8.

## Feito

- [x] **0. Base:** Tauri v2 + Vite + JS puro, os três estilos do Jay-C (Calm, Neon, Soft), instância única; abrir pelo diálogo, soltando, pela linha de comando ou por um segundo lançamento; detecção do tipo de arquivo; uma aba por arquivo.
- [x] **1. Leitor de PDF:** desenhado no Rust com o hayro, só as páginas visíveis; zoom, miniaturas, ir para página, buscar, copiar texto, links, senha. Um PDF de 1 000 páginas rola a 72 quadros/s.
- [x] **2. Markdown e texto:** CodeMirror com prévia ao vivo feita no Rust (fórmulas incluídas, limpa pelo ammonia); salvar mantém a codificação e o fim de linha do arquivo, de forma atômica; exportar para HTML e PDF.
- [x] **3. Imagens:** edições que não destroem o original (cortar, girar, espelhar, brilho/contraste/saturação) com desfazer; converter em lote para WebP, AVIF, JPEG, PNG e outros; fotos de 100 MP; bombas de descompressão recusadas.
- [x] **4. Documentos:** um modelo de documento com leitores de DOCX, ODT, RTF, HTML e Markdown; vista em páginas A4; exportar para DOCX, Markdown, HTML, texto e PDF.
- [x] **5. Vídeo:** cortar e juntar MP4/MOV sem recodificar (10 s de um arquivo de 1 GB em 42 ms), faixa de miniaturas, salvar quadros em PNG, converter para MP4 ou M4A pelo Media Foundation do Windows.
- [x] **Atualizações:** o Jay-E confere o último release no GitHub ao abrir e oferece "Atualizar para x.y.z"; o instalador é conferido contra o SHA-256 do GitHub, edições não salvas são perguntadas, e o Jay-E reabre no fim. Os releases são gerados pela CI num tag de versão.
- [x] **6. LaTeX:** compilado para PDF pelo motor do TeX do Tectonic dentro do `jay-e.exe` (rodado como processo filho, sem programa baixado), o PDF ao lado do editor no leitor da etapa 1, erros que levam à linha. Um artigo com bibliografia: 1,4 s (98 s na primeira vez, baixando 42 MB de pacotes do TeX).

## A seguir

- [ ] **7. Edição:** DOCX e ODT (texto, estilos, listas, tabelas simples, imagens) com ida e volta sem perda; PDF: juntar, separar, reordenar, girar e apagar páginas, anotações, formulários.
  - [x] DOCX editado no lugar com uma barra de processador de texto, na própria página, fontes e estilos do arquivo; salvar reescreve só os parágrafos que mudaram (o Word abre o resultado sem reparos).
  - [x] ODT do mesmo jeito (a mesma barra, imagens, tabelas, listas, cabeçalho e rodapé); RTF continua só leitura (exportar para DOCX).
  - [x] Imagens redimensionadas e movidas livremente em qualquer lugar da página; inserir imagens (arquivo ou Ctrl+V) e tabelas; cabeçalhos e rodapés; a página dividida nas páginas do arquivo; Ctrl+Z/Ctrl+Y para tudo; colar com formatação.
  - [x] PDF: reordenar, girar, apagar, inserir outro PDF e salvar páginas à parte; destacar, desenhar, notas, texto, borracha; preencher formulários; desfazer e salvar.
  - [x] Passada de interface na marca Jay (do Jay-C): as barras de edição nos grupos segmentados do Jay-C, no raio de cada estilo, os botões ligados na cor de destaque; o ícone da janela e da barra de tarefas é o lobo nas cores do estilo e muda com ele, também em execuções de dev iniciadas de dentro do pacote de outro app.
  - [x] Linhas e colunas nas tabelas do arquivo (inserir acima/abaixo/à esquerda/à direita, apagar); um cabeçalho ou rodapé onde o arquivo não tinha; tabelas longas divididas linha por linha entre páginas, como no Word.
  - [x] Cabeçalhos e rodapés diferentes na primeira página e nas pares ("Primeira página diferente", "Páginas pares e ímpares diferentes"); a linha de título de uma tabela repetida no topo de cada página em que ela continua.
- [x] **8. Conversor:** uma tela de lote para todos os tipos de arquivo (imagens, DOCX/ODT/RTF/Markdown, PDF para texto, vídeo e áudio), um formato por tipo, uma fila com progresso por arquivo, Parar, e pergunta antes de sobrescrever (manter os dois, substituir, pular). 20 arquivos mistos em 3 s; o danificado falhou sozinho e os outros 18 saíram.
- [x] **9. Lançamento:** [v0.1.0](https://github.com/TyxMatty/Jay-E/releases/tag/v0.1.0) publicado em 07/11/2026: instalador NSIS em `%LOCALAPPDATA%\Jay-E` (17 MB), associações de arquivo ("Abrir com"), o Jay-E no catálogo do Jay-H. Instalado, um PDF aberto com dois cliques, e o Jay-H achou o Jay-E.
- [x] **10. Mais ferramentas:** os editores de imagem e de vídeo crescem (ideias do PhotoCraft e do OpenShot, [`docs/REFERENCIAS.md`](docs/REFERENCIAS.md) seção 6), e mais ideias para cada tipo de arquivo.
  - [x] Imagens, tom e cor: Curvas e Níveis com histograma; exposição, realces/sombras, temperatura e matiz, vibração; preto e branco; tom, contraste e cor automáticos. Tudo na lista de edições, com desfazer.
  - [x] Marca Jay: o tour de boas-vindas e os diálogos animam como no Jay-C (Configurações › Aparência › Animações, Ligadas/Desligadas), e o brilho de destaque de todo app Jay fica atrás das seções.
  - [x] Imagens, filtros: nitidez, desfoque, reduzir ruído, vinheta; visuais de cor de arquivos LUT `.cube`.
  - [x] Imagens, geometria: endireitar (giro livre com grade), cortar numa proporção (1:1, 4:3, 16:9…), corte em perspectiva.
  - [x] Imagens, desenho: caneta e marca-texto em qualquer cor e tamanho, sobre a imagem; cada traço desfaz sozinho e acompanha giros, espelhamentos e cortes feitos depois.
  - [x] Imagens, anotações: texto, setas, retângulos e elipses, borrar ou pixelar uma área (esconder dados).
  - [x] Imagens: olhos vermelhos; remover mancha.
  - [x] Imagens: Exportar com prévia e o tamanho do arquivo antes de salvar.
  - [x] Vídeo, linha do tempo: miniaturas e a forma de onda do som, zoom, quadro a quadro (, e .).
  - [x] Vídeo, navalha: dividir em vários trechos, tirar os do meio, reordenar; copiado sem recodificar.
  - [x] Vídeo, velocidade (0,25× a 4×); um formato de quadro (1:1, 9:16, 16:9, 4:5), girar 90°, espelhar.
  - [x] Vídeo: tocar ao contrário.
  - [x] Vídeo, som: volume, sem som, entrada e saída graduais, salvar só o som.
  - [x] Vídeo: trocar o som ou pôr música.
  - [x] Vídeo, entradas e saídas do preto; texto e uma imagem de marca d'água sobre o vídeo, de um tempo a outro; brilho, contraste, saturação.
  - [x] Vídeo: dissolver entre os trechos da navalha; borrar uma área.
  - [x] Vídeo: predefinições de exportação (WhatsApp, YouTube 1080p, arquivo pequeno, só o som).
  - [x] Vídeo: GIF animado.
  - [x] Imagens: as mesmas edições em várias imagens de uma vez; ler HEIC e RAW pelos codecs do próprio Windows (WIC); ver os detalhes da foto (lugar, câmera) e salvar sem eles; antes/depois.
  - [x] Vídeo: encolher para um tamanho (25 MB para o Discord, 16 MB para o WhatsApp).
  - [x] Vídeo: legendas (`.srt`, `.vtt`) mostradas no player e gravadas no vídeo; salvar um quadro a cada N segundos.
  - [x] PDF: comprimir (imagens em 150 dpi no maior tamanho em que a página as mostra, em JPEG quando fica menor); uma assinatura desenhada, guardada para a próxima vez e posta com um clique; tarjar de verdade (o texto, os pixels das imagens e as anotações embaixo da caixa saem do arquivo); deixar pesquisável com o OCR do próprio Windows (`Windows.Media.Ocr`); imagens num PDF só e páginas como imagens (no conversor).
  - [x] PDF, editar: mudar o texto do próprio PDF (palavras e frases trocadas no lugar, na fonte do arquivo, e o texto antigo removido de verdade); preencher os espaços de qualquer PDF, mesmo sem campos de formulário (escrever em linhas e caixas, marcar ✓ ou ✗, data), com os campos achados sozinhos; trocar, mover ou apagar uma imagem da página.
  - [x] Documentos: localizar e substituir (Ctrl+F, Ctrl+H, desfeito num passo só); contar palavras; ortografia no idioma do arquivo pelo corretor do Windows, com sugestões no clique direito; comparar com uma versão anterior, parágrafo por parágrafo e palavra por palavra.
  - [x] Conversor: PDF para DOCX (os parágrafos, títulos, negrito e itálico, imagens e quebras de página refeitos a partir das linhas do PDF); ler HTML; LaTeX para PDF; páginas de PDF como PNG ou JPEG; imagens num PDF só; uma pasta vigiada, cujos arquivos novos são convertidos quando chegam.

## Pendências (achadas no caminho)

- [ ] Conversor: testar o Parar no meio de um lote no app.

- [x] Publicar o primeiro release: [v0.1.0](https://github.com/TyxMatty/Jay-E/releases/tag/v0.1.0) em 07/11/2026 (instalador de 17 MB, SHA-256 no GitHub).
- [ ] Testar a atualização de uma versão para a seguinte: instalar o v0.1.0 e deixar ele achar o v0.2.0.
- [x] As imagens dos documentos e o visualizador de imagens não carregavam no app instalado (o CSP não deixava `<img>` usar o `jaye:`; o modo de dev não aplica o CSP). Consertado depois do v0.2.0.
- [ ] Conferir cada release num build de release (`npx tauri build --no-bundle`) antes de publicar: o CSP só vale ali.
- [ ] PDF para DOCX: fórmulas em linha própria (frações, somatórios) saem em pedaços; tabelas viram o texto das linhas.
- [ ] PDF: tarjar texto que está dentro de grupos (Form XObjects); hoje a tarja é recusada ali, sem mudar nada.
- [ ] Ortografia: um documento em mais de um idioma é conferido só no idioma do arquivo; sem dicionário do Windows para ele, vale o corretor da janela.
- [ ] Medir a primeira página do leitor de PDF num build de release (a meta de 150 ms passa em 3 de 5 PDFs em dev).
- [ ] Testar um PDF com senha de usuário, um link externo de um PDF, e leitores de tela nas páginas.
- [ ] O tema claro no leitor de PDF, nos editores e na vista de documentos.
- [ ] Vídeo: HEVC, um vídeo de celular gravado de lado (rotação), o progresso e o Parar da conversão no app; um M4A feito pelo Media Foundation abre como vídeo.
- [ ] Documentos: um DOCX grande (centenas de páginas); ODT e RTF salvos pelo Word.
- [ ] Markdown: um arquivo de 50 MB no editor; a confirmação ao fechar com edições não salvas, conferida no app.
- [ ] LaTeX: compilar sem internet depois da primeira vez, "Salvar PDF…", Parar e o tempo limite no app, um documento com imagens.
- [ ] O instalador cresceu para 16,5 MiB com o motor do TeX (meta: 15 MB): ver se os dados do ICU podem ser cortados ao que o XeTeX usa.
- [ ] CI que roda `npm test` e os dois `cargo test` a cada push.

## Em aberto

- Planilhas (XLSX/ODS/CSV) e apresentações (PPTX/ODP): fora do v1 por enquanto ("não suportado"); só leitura depois da etapa 8 precisaria de um modelo de células.
- Leitores num processo separado: só se um PDF algum dia travar o interpretador.
