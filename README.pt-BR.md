# Jay-E

[English](README.md) · **Português**

O editor e leitor da linha Jay: documentos (PDF, DOCX, ODT, RTF, Markdown, LaTeX, texto), imagens e vídeos, com conversão entre formatos. Leve, rápido para desenhar, Rust primeiro. Windows primeiro.

É para quem quer um app só, pequeno e rápido, para abrir, ajeitar e converter os arquivos do dia a dia (um PDF para ler, um DOCX para virar Markdown, um lote de fotos para encolher em WebP, um trecho para cortar de um vídeo longo) sem instalar uma suíte de escritório, um editor de fotos e um editor de vídeo. Faz parte da linha Jay com o [Jay-C](https://github.com/TyxMatty/Jay-C) (chamadas), o [Jay-A](https://github.com/TyxMatty/Jay-A) (áudio) e o [Jay-H](https://github.com/TyxMatty/Jay-H) (o hub que instala todos).

## O que já faz (etapa 6)

- **Abrir arquivos:** pelo botão Abrir (Ctrl+O), soltando na janela ou pela linha de comando, um por aba.
- **PDF:** o leitor do próprio Jay-E, desenhado no Rust com o hayro: miniaturas, zoom, busca, copiar texto, links, PDFs com senha.
- **Markdown, LaTeX e texto:** um editor que salva na codificação e no fim de linha do próprio arquivo; o Markdown com prévia ao vivo renderizada no Rust (com fórmulas) e exportação para HTML e PDF; o LaTeX compilado em PDF ao lado do editor pelo motor do TeX dentro do Jay-E (o do Tectonic), com erros que levam à linha.
- **Imagens:** girar, espelhar, cortar, brilho/contraste/saturação, desfazer, zoom em fotos de 100 MP; converter uma ou todas de uma vez para WebP, AVIF, JPEG, PNG e outros.
- **Vídeo e áudio:** tocam com uma faixa de miniaturas; cortar e juntar sem recodificar (na hora, mesmo num arquivo de 1 GB); converter em MP4 ou M4A pelo Media Foundation do Windows; salvar quadros em PNG.
- **Word (DOCX), OpenDocument (ODT) e RTF:** numa folha A4, exportados para DOCX, Markdown, HTML, texto e PDF.
- **Atualizações:** ao abrir, o Jay-E confere a release mais nova no GitHub e oferece se atualizar (ver [Atualizações](#atualizações)).
- A janela vem nos estilos Calmo, Neon e Suave do Jay-C, clara ou escura, em português ou inglês.

O que vem a seguir e a lista de pendências: [`ROADMAP.md`](ROADMAP.md) (em inglês). Plano e etapas completos: [`docs/ARQUITETURA.md`](docs/ARQUITETURA.md); o estudo do LibreOffice, ONLYOFFICE, MarkText, PhotoCraft e Kdenlive que embasa o plano: [`docs/REFERENCIAS.md`](docs/REFERENCIAS.md).

## Feito com

- **Rust** em tudo que lê ou grava arquivo (`crates/jay-docs`): [hayro](https://github.com/LaurenzV/hayro) para PDF, pulldown-cmark + ammonia para Markdown, a crate `image`, ravif e libwebp para imagens, e o Media Foundation do Windows para vídeo.
- **Tauri 2** para a janela (`frontend/src-tauri`), com a WebView2.
- **JavaScript puro** + Vite na interface (`frontend/src`), com o CodeMirror 6 nos editores.

## Rodando

Node 22+ e Rust com MSVC (ver `docs/CONTINUAR.md` do Jay-C), e o [vcpkg](https://github.com/microsoft/vcpkg) com as bibliotecas em C do motor de LaTeX (uma vez; o ICU leva uns 20 min):

```bash
vcpkg install --triplet x64-windows-static-md icu freetype graphite2 fontconfig libpng
```

```bash
npm install
npm run dev
```

`npm run dev` abre a janela do Jay-E (Tauri) com recarga automática. `jay-e.exe <arquivo>…` abre arquivos; com o Jay-E já aberto, eles vão para a janela dele. Para abrir só a interface no navegador, sem Tauri: `npm run web` (porta 5177; não abre arquivos).

Crie um `.cargo/config.toml` (fora do git) com a pasta do build fora do OneDrive e onde está o vcpkg:

```toml
[build]
target-dir = "C:/Users/<você>/.cargo-target/jay-e"

[env]
TECTONIC_DEP_BACKEND = "vcpkg"
VCPKG_ROOT = "C:/Users/<você>/vcpkg"
VCPKGRS_TRIPLET = "x64-windows-static-md"
```

`npm run build` gera o instalador do Windows (NSIS) em `frontend/src-tauri/target/release/bundle/nsis/`.

## Testes

```bash
npm test
cd crates/jay-docs && cargo test
cd frontend/src-tauri && cargo test
```

Para testar o app de verdade: abra-o com `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9226` e rode `node scripts/cdp-eval.mjs <arquivo.js>`, que executa o arquivo dentro da página dele.

## Atualizações

Ao abrir, o Jay-E pergunta ao GitHub pela release mais nova deste repositório. Se ela for mais nova que a versão em uso, aparece um botão "Atualizar pra x.y.z" na barra de cima. O clique baixa o `*_x64-setup.exe` da release, que só roda se o SHA-256 bater com o que o GitHub calculou, pergunta sobre edições não salvas, roda o instalador em modo passivo (mantendo as configurações) e reabre o Jay-E. É a mesma checagem que o Jay-H usa para instalar os apps Jay; sem internet, nada aparece.

Para lançar uma versão:

1. Suba a versão em `frontend/src-tauri/tauri.conf.json`, `frontend/src-tauri/Cargo.toml` e `frontend/package.json`.
2. Crie a tag e envie: `git tag v0.2.0 && git push origin v0.2.0`.
3. O workflow **Installer** gera o instalador e cria uma release em rascunho com ele. Escreva as notas, teste o instalador e publique a release: a partir daí, todo Jay-E a encontra.

## Licença

[MIT](LICENSE).
