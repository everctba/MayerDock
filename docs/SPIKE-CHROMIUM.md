# Spike md-9va — Chromium: throttle/oclusão e WebGPU/WebGL2/rAF

Gerado por `scripts/chromium_throttle_spike.py` via CDP (browser lançado pelo
`browser_launch` do MayerDock). Métricas: rAF em ~2,1 s, timers de 100 ms,
WebGL2 (renderer), WebGPU (adapter), `visibilityState` e foco.

| Cenário | rAF (2,1 s) | timers 100 ms | WebGL2 | WebGPU | visibility | foco |
| --- | --- | --- | --- | --- | --- | --- |
| headed_offscreen | 197 | 21 | True | False | visible | False |
| headed_visible | 202 | 21 | True | False | visible | False |
| headed_minimized | 0 | 21 | True | False | hidden | False |
| headless_new | 160 | 21 | True | False | visible | False |
| headed_offscreen_webgpu_flag | 162 | 21 | True | False | visible | False |

## Renderers / adapters

- **headed_offscreen**: WebGL2 renderer = `ANGLE (NVIDIA, NVIDIA GeForce RTX 4060 Ti (0x00002803) Direct3D11 vs_5_0 ps_5_0, D3D11)`; WebGPU adapter = `None`
- **headed_visible**: WebGL2 renderer = `ANGLE (NVIDIA, NVIDIA GeForce RTX 4060 Ti (0x00002803) Direct3D11 vs_5_0 ps_5_0, D3D11)`; WebGPU adapter = `None`
- **headed_minimized**: WebGL2 renderer = `ANGLE (NVIDIA, NVIDIA GeForce RTX 4060 Ti (0x00002803) Direct3D11 vs_5_0 ps_5_0, D3D11)`; WebGPU adapter = `None`
- **headless_new**: WebGL2 renderer = `ANGLE (NVIDIA, NVIDIA GeForce RTX 4060 Ti (0x00002803) Direct3D11 vs_5_0 ps_5_0, D3D11)`; WebGPU adapter = `None`
- **headed_offscreen_webgpu_flag**: WebGL2 renderer = `ANGLE (NVIDIA, NVIDIA GeForce RTX 4060 Ti (0x00002803) Direct3D11 vs_5_0 ps_5_0, D3D11)`; WebGPU adapter = `None`

## Leitura

- rAF alto (197/202 em 2,1 s) = sem throttle; **0** = parado (minimizado).
- As flags `--disable-backgrounding-occluded-windows`,
  `--disable-renderer-backgrounding`, `--disable-background-timer-throttling` e
  `--disable-features=CalculateNativeWinOcclusion` são o baseline do `browser_launch`.
- Minimizado é o pior caso: rAF caiu a **0** (`visibilityState=hidden`) mesmo com as
  flags, mas os timers de 100 ms continuaram ~21 → as flags anti-background-timer
  funcionam, porém rAF parado inviabiliza animação/WebGPU. Manter a janela
  **offscreen visível**, nunca minimizada.
- Headless `new` neste ambiente usou a **GPU real** (ANGLE/D3D11) e rAF 160 — mas isso
  varia por máquina; medir antes de confiar.
- **WebGPU: indisponível mesmo com `--enable-unsafe-webgpu`** (`requestAdapter` nulo em
  todos os cenários). O `browser_launch` aceita `extra_flags` para testes; antes de
  depender de WebGPU, validar versão do Chrome/driver e repetir o spike.
- WebGL2 é confiável em todos os cenários (ANGLE NVIDIA D3D11).
