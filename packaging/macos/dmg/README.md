# Janela do instalador macOS (DMG)

`background.svg` é o fundo da janela do DMG da Nova Foods: cor do app, logo Nova Foods (marca
registrada, termos em `docs/brand/LICENSE-brand.txt`), nome do app e a instrução de instalação. O
texto é Montserrat (OFL) já convertido em contornos, então nenhuma fonte fica no repositório.

What Finder shows when the DMG opens: a 660 × 400 pt background with the app icon and the
`Applications` link side by side. `package.sh` copies these files into the image; nothing here is
generated at build time, so the DMG builds with a plain `hdiutil create -srcfolder` (no Finder
scripting on CI, nothing extra installed in the signing job).

Nothing in this step may touch `Nova Word.app`: `codesign --verify --deep --strict` rejects any
`com.apple.FinderInfo` or resource fork on the bundle. `hdiutil makehybrid -hfs`, which built the
image before, put a FinderInfo on every file in it, so the released app failed that check (#525).
`package.sh` clears extended attributes before signing and, after building the DMG, mounts it
read-only and checks the app inside it, and a `ditto` copy of it, with `--strict`.

`generate.py` desenha o `background.tiff` (1x e 2x) e o `dmg-layout.DS_Store` (tamanho da janela,
posição dos ícones) a partir dele:

    pip install 'pillow>=12' ds_store==1.3.3 mac_alias==2.2.3
    python3 packaging/macos/dmg/generate.py      # precisa do resvg no PATH

O app fica em (326, 205) e o atalho Aplicativos em (574, 205); a seta do fundo fica entre os dois.
