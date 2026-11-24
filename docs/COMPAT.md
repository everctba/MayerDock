# Matriz de compatibilidade — captura e UIA (spike md-tkr)

Gerado por `scripts/compat_matrix.py` em 2026-11-24 20:53.
Ambiente: Windows 10 19041 (máquina de desenvolvimento); janelas abertas pelo próprio teste.

Metodologia: para cada janela, `capture_window` em duas variantes (PrintWindow com
`PW_RENDERFULLCONTENT` e BitBlt forçado), `list_controls` (EnumChildWindows) e
`uia_query` (UI Automation, máximo 120 elementos). “Não-preto” = % de pixels com
luminância > 8 (detecta frame preto). “Cores” = cores únicas numa miniatura 64×64.

| App | HWND | PrintWindow | BitBlt | Controles filhos | Elementos UIA | Observações |
| --- | --- | --- | --- | --- | --- | --- |
| Notepad (Win32 clássico) | 0xE12B8 | 447x873, não-preto 96%, brilho 242, cores 77 | 447x873, não-preto 100%, brilho 249, cores 119 | 2 | 2 | ok |
| Calculadora (UWP/XAML) | 0x613E4 | 399x654, não-preto 95%, brilho 229, cores 149 | 399x654, não-preto 0%, brilho 0, cores 1 | 4 | 49 | BitBlt preto |
| Explorer (Win32 shell) | - | janela não encontrada | - | - | - | não abriu |
| WordPad (Win32 + rich edit) | 0xD1254 | 1157x1096, não-preto 97%, brilho 236, cores 127 | 1157x1096, não-preto 97%, brilho 236, cores 145 | 25 | 20 | ok |
| Chrome (Chromium, já aberto) | 0x106E4 | 536x828, não-preto 97%, brilho 40, cores 163 | 536x828, não-preto 96%, brilho 30, cores 8 | 2 | -1 | UIA falhou |

## Notas

- Chromium/Electron renderizam por GPU: PrintWindow pode devolver frame preto
  (WGC é o caminho recomendado); BitBlt só pega o que está visível na tela.
- UWP/XAML expõe árvore UIA rica (bom para `uia_invoke`); apps Win32 clássicos
  variam (Notepad não expõe a menu bar como elemento UIA).
- Calculadora: BitBlt devolve frame preto (superfície composta), PrintWindow ok;
  é o caso que justifica WGC para apps GPU.
- Para browser, a estratégia do MayerDock é `browser_launch` + **CDP**, não captura
  de janela.
- Este documento é um retrato do ambiente; medir de novo após o UIA nativo (COM).
