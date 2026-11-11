# Continuar o Jay-E em outro PC

**Português** · [English](CONTINUAR.en.md)

Atualizado em 2026-11-10, depois do 0.2.2: o setup do Jay-I. Tudo o que é preciso para montar o Jay-E do zero, rodar, entender como cada parte funciona e saber o que vem a seguir. O plano completo, com as medições de cada etapa, está em [`ARQUITETURA.md`](ARQUITETURA.md).

## 0. No PC que já tem o projeto

```powershell
git pull
npm install
npm test
```

Se o `pull` trouxer mudança no LaTeX e o vcpkg ainda não tiver as bibliotecas dele, rode o passo do vcpkg da seção 1. Depois, `npm run dev`.

## 1. Instalar as ferramentas (Windows)

Em PowerShell. Cada `winget` pode pedir aprovação de administrador (UAC).

```powershell
winget install -e --id Git.Git
winget install -e --id OpenJS.NodeJS.LTS
winget install -e --id Microsoft.VisualStudio.2022.BuildTools --override "--wait --passive --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
winget install -e --id Rustlang.Rustup
```

Depois, **num terminal novo**, o **vcpkg** com as bibliotecas em C do motor de LaTeX (ICU, FreeType, Graphite2, fontconfig e libpng, todas estáticas). A primeira vez leva uns 30 minutos; o ICU é quase tudo.

```powershell
git clone https://github.com/microsoft/vcpkg C:\Users\<você>\vcpkg
C:\Users\<você>\vcpkg\bootstrap-vcpkg.bat -disableMetrics
C:\Users\<você>\vcpkg\vcpkg install --triplet x64-windows-static-md icu freetype graphite2 fontconfig libpng
```

O WebView2 já vem no Windows 11. Nada de Python, LibreOffice, FFmpeg ou TeX instalado: o Jay-E não usa nenhum deles.

## 2. Pegar o código

Clone **fora do OneDrive**. O build do Rust gera vários GB e o OneDrive tentaria sincronizá-los.

```powershell
git clone https://github.com/TyxMatty/Jay-E.git C:\dev\Jay-E
cd C:\dev\Jay-E
npm install
```

Crie `.cargo\config.toml` na raiz do repositório (é local e não vai para o git). O `[env]` diz ao build onde estão as bibliotecas do vcpkg; o `[build]` só é preciso se o clone ficar dentro do OneDrive:

```toml
[build]
target-dir = "C:/Users/<você>/.cargo-target/jay-e"

[env]
TECTONIC_DEP_BACKEND = "vcpkg"
VCPKG_ROOT = "C:/Users/<você>/vcpkg"
VCPKGRS_TRIPLET = "x64-windows-static-md"
```

## 3. Rodar

```powershell
npm run dev        # abre o app (o 1º build compila o motor do TeX; leva uns 10 minutos)
npm run web        # só a interface no navegador, sem Tauri (porta 5177; não abre arquivos)
npm run build      # o instalador NSIS, em <target-dir>\release\bundle\nsis\ (o setup da release, que leva o NSIS dentro, só sai na CI, ou à mão com o scripts/build-setup.ps1 do Jay-I)
```

Testes:

```powershell
npm test
cd crates/jay-docs; cargo test
cd frontend/src-tauri; cargo test
```

**Comandar o app de verdade:** abra-o com a depuração remota ligada; aí um arquivo `.js` roda dentro da página dele, com os comandos do Rust funcionando:

```powershell
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=9226'; npm run dev
node scripts/cdp-eval.mjs teste.js
node scripts/screenshots.mjs artigo.tex notas.md   # refaz os prints do README
```

`jay-e.exe <arquivo>…` abre arquivos; com o Jay-E já aberto, eles vão para a janela dele (instância única).

## 4. Como o Jay-E funciona

Duas camadas: o **engine** em Rust ([`crates/jay-docs`](../crates/jay-docs), sem Tauri), que lê, desenha, edita e converte; e o **app** ([`frontend`](../frontend)), que é a janela Tauri (`src-tauri`, Rust) e a página em JS puro (`src`), que só mostra e manda comandos.

| Parte | Onde | Como funciona |
|---|---|---|
| Abrir arquivos | `detect.rs`, `src-tauri/src/files.rs` | O tipo é decidido pelos primeiros 4 KB, não pela extensão. Cada arquivo aberto (pelo diálogo, soltando, pela linha de comando ou por um segundo lançamento) entra numa lista no Rust; a página nunca manda um caminho que não esteja nela. |
| PDF | `pdf.rs`, `src-tauri/src/pdf.rs` | O hayro (Rust puro) desenha numa thread por documento. As páginas vão para a página como PNG pelo protocolo `jaye://`, só as visíveis. Busca, texto e links também saem do Rust. |
| Markdown e texto | `text.rs`, `markdown.rs` | Salva na mesma codificação e no mesmo fim de linha do arquivo, de forma atômica, e avisa se ele mudou por fora. A prévia é feita no Rust (pulldown-cmark, fórmulas em MathML) e limpa pelo ammonia, então `<script>` no Markdown não roda. |
| LaTeX | `latex.rs`, `src-tauri/src/latex.rs` | O motor do Tectonic (XeTeX) vem dentro do `jay-e.exe`. Para compilar, o app roda a si mesmo com `--jaye-tex` num processo à parte, sem janela, em modo não confiável, com tempo limite e Parar. Só os pacotes do TeX vêm da internet, na primeira vez (42 MB para um artigo), e ficam em cache. Os erros viram uma lista que leva à linha. |
| Imagens | `image.rs` | Decodificadas no Rust, com limites contra bombas de descompressão. As edições são uma lista que se aplica por cima do original (dá para desfazer); converter usa a libwebp para WebP e o ravif para AVIF. |
| Editar DOCX e ODT | `doc/edit.rs`, `doc/odt_edit.rs`, `ui/docx-editor.js` | A página é montada com as fontes, estilos, margens e páginas do próprio arquivo; cada parágrafo sabe onde está no XML. Salvar reescreve só os parágrafos que mudaram (e o cabeçalho/rodapé, se mudaram); o resto do pacote é copiado byte a byte. Imagens soltas viram flutuantes onde caem. |
| Editar PDF | `pdf_edit.rs`, `src-tauri/src/pdf_edit.rs`, `ui/pdf-tools.js` | Cada mudança (páginas, anotações, formulário) gera um PDF novo pelo lopdf, que o hayro abre; desfazer volta uma versão; salvar grava a atual. |
| DOCX, ODT, RTF | `doc/` | Um modelo de documento próprio no meio: cada formato é lido para ele e escrito a partir dele (DOCX, Markdown, HTML, texto). A página mostra numa folha A4; o PDF sai pela impressão da WebView2. |
| Conversor | `src-tauri/src/convert.rs`, `ui/convert.js` | Uma fila numa thread: cada arquivo vai para o conversor do seu tipo (imagem, documento, PDF para texto, vídeo), é gravado num temporário e renomeado no fim, e conta o resultado sozinho; um erro não para os outros. A pasta é escolhida no Rust. |
| Vídeo | `video/mp4.rs`, `video/mf.rs` | Cortar e juntar copiando as amostras do MP4, sem recodificar (o `mp4-atom` reescreve as tabelas). Converter e tirar quadro passam pelo Media Foundation do Windows, que usa a GPU quando há. |
| Atualizações | `src-tauri/src/update.rs` | Ao abrir, confere o release mais novo no GitHub pela crate `jay-i` da linha (git, fixada por commit; a mesma do Jay-S); o setup só roda se tamanho e SHA-256 baterem com os do GitHub. |
| Estilos e idioma | `frontend/src/styles`, `locales` | Os estilos Calmo, Neon e Suave do Jay-C, claro ou escuro; inglês ou português. |

## 5. Estado atual

| Etapa | O que entrega |
|---|---|
| 0. Base | Janela, estilos, tema, idioma, instância única; abrir de quatro jeitos; detecção do tipo; uma aba por arquivo |
| 1. PDF | Leitor desenhado no Rust: só as páginas visíveis, zoom, miniaturas, ir para página, buscar e copiar texto, links, senha. Um PDF de 1 000 páginas rola a 72 quadros/s |
| 2. Markdown | Editor com prévia ao vivo e fórmulas; salvar seguro; exportar HTML e PDF |
| 3. Imagens | Cortar, girar, espelhar, ajustes, desfazer; converter em lote; fotos de 100 MP |
| 4. Documentos | DOCX, ODT, RTF, HTML e Markdown numa folha A4; exportar para DOCX, Markdown, HTML, texto e PDF |
| 5. Vídeo | Cortar e juntar sem recodificar (10 s de um arquivo de 1 GB em 42 ms); converter em MP4 ou M4A; salvar quadro |
| Atualizações | Confere o GitHub ao abrir e se atualiza; a CI gera o instalador num tag de versão |
| 6. LaTeX | Compila com o motor do Tectonic dentro do app: 1,4 s para um artigo com bibliografia (98 s na primeira vez) |
| 7. Edição | DOCX e ODT editados com a barra de sempre (imagens soltas, tabelas, cabeçalhos da primeira página e das pares, linha de título repetida, páginas, Ctrl+Z); PDF: páginas, anotações e formulários |
| 8. Conversor | Uma tela para converter arquivos de todos os tipos numa pasta, um formato por tipo, com fila, progresso, Parar e pergunta antes de substituir; 20 arquivos mistos em 3 s, o estragado falha sozinho |
| 9. Lançamento | O v0.1.0 publicado: instalador NSIS, "Abrir com" para todos os tipos, o Jay-E no Jay-H |
| 10. Mais ferramentas | Imagens (curvas, filtros, endireitar, anotar, retoque, HEIC/RAW), vídeo (linha do tempo, navalha, velocidade, som, texto, legendas, GIF), PDF (editar o texto, preencher, imagens, tarja, assinatura, comprimir, OCR), documentos (localizar e substituir, palavras, ortografia, comparar), conversor (HTML, LaTeX, páginas como imagens, imagens num PDF, pasta vigiada). Saiu no v0.2.0 |
| 0.2.2. Setup do Jay-I | O arquivo da release agora é o setup do Jay-I, que leva o instalador NSIS dentro e o roda em silêncio; o atualizador deixou de ser uma cópia própria e é a crate `jay-i` da linha (detalhes e os 10 pontos na seção 7 da `ARQUITETURA.md`) |

**Tamanho:** o instalador NSIS tem 17,59 MiB (18 446 431 bytes, medido no 0.2.2), acima da meta de 15 MB por causa do motor do TeX (os dados do ICU são quase tudo); o setup publicado tem 20,67 MiB (21 674 496 bytes, montado aqui com o mesmo passo do workflow; o NSIS está dentro dele, byte a byte) (teto de 30 MB na CI).

**Próximas etapas:** nenhuma planejada; as pendências abaixo e as ideias de `docs/REFERENCIAS.md` (seção 6) que ficaram de fora.

**Pendências** (a lista completa está no [`ROADMAP.md`](../ROADMAP.md)):
- testar a atualização do v0.1.0 para o v0.2.0 numa instalação de verdade;
- LaTeX: compilar sem internet depois da primeira vez, o "Salvar PDF…", o Parar e o tempo limite no app;
- ver se os dados do ICU podem ser cortados para o que o XeTeX usa, para o instalador voltar aos 15 MB;
- uma CI que rode `npm test` e os dois `cargo test` a cada push.
