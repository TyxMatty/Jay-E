# Jay-E

[English](README.md) · **Português**

O editor e leitor da linha Jay: documentos (PDF, DOCX, ODT, RTF, Markdown, LaTeX, texto), imagens e vídeos, com conversão entre formatos. Leve, rápido para desenhar, Rust primeiro. Windows primeiro.

Plano e etapas: [`docs/ARQUITETURA.md`](docs/ARQUITETURA.md); o estudo do LibreOffice, ONLYOFFICE, MarkText, PhotoCraft e Kdenlive que embasa o plano: [`docs/REFERENCIAS.md`](docs/REFERENCIAS.md). Por enquanto (etapa 0) o app tem a janela (nos estilos Calmo, Neon e Suave do Jay-C) e abre arquivos pelo botão Abrir (Ctrl+O), soltando na janela ou pela linha de comando, um por aba. Imagens, vídeo, áudio e texto aparecem; PDF, DOCX, Markdown e LaTeX ganham os visualizadores nas próximas etapas.

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
