# Jay-E

[English](README.md) · **Português**

O editor e leitor da linha Jay: documentos (PDF, DOCX, ODT, RTF, Markdown, LaTeX, texto), imagens e vídeos, com conversão entre formatos. Leve, rápido para desenhar, Rust primeiro. Windows primeiro.

Plano e etapas: [`docs/ARQUITETURA.md`](docs/ARQUITETURA.md); o estudo do LibreOffice, ONLYOFFICE, MarkText, PhotoCraft e Kdenlive que embasa o plano: [`docs/REFERENCIAS.md`](docs/REFERENCIAS.md). Por enquanto (etapa 3) o app tem a janela (nos estilos Calmo, Neon e Suave do Jay-C) e abre arquivos pelo botão Abrir (Ctrl+O), soltando na janela ou pela linha de comando, um por aba. PDFs abrem no leitor do próprio Jay-E, desenhado no Rust com o hayro (miniaturas, zoom, busca, copiar texto, links); Markdown, LaTeX e texto abrem num editor que salva na codificação e no fim de linha do próprio arquivo, o Markdown com prévia ao vivo renderizada no Rust (com fórmulas) e exportação para HTML e PDF; imagens abrem num editor (girar, espelhar, cortar, brilho/contraste/saturação, desfazer, zoom em fotos de 100 MP) e se convertem, uma ou todas de uma vez, para WebP, AVIF, JPEG, PNG e outros; vídeo e áudio tocam; o DOCX ganha o visualizador em seguida.

## Rodando

Node 22+ e Rust com MSVC (ver `docs/CONTINUAR.md` do Jay-C).

```bash
npm install
npm run dev
```

`npm run dev` abre a janela do Jay-E (Tauri) com recarga automática. `jay-e.exe <arquivo>…` abre arquivos; com o Jay-E já aberto, eles vão para a janela dele. Para abrir só a interface no navegador, sem Tauri: `npm run web` (porta 5177; não abre arquivos).

Para o build do Rust ficar fora do OneDrive, crie um `.cargo/config.toml` (fora do git):

```toml
[build]
target-dir = "C:/Users/<você>/.cargo-target/jay-e"
```

## Testes

```bash
npm test
cd crates/jay-docs && cargo test
cd frontend/src-tauri && cargo test
```

Para testar o app de verdade: abra-o com `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9226` e rode `node scripts/cdp-eval.mjs <arquivo.js>`, que executa o arquivo dentro da página dele.

## Licença

MIT.
